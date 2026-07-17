use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tracing::{info, warn};

use csa_config::ProjectConfig;
use csa_core::types::{OutputFormat, ToolName};

use crate::codex_transcript_filter::{
    extract_codex_json_event_text, first_non_empty_line_is_thread_started,
};
use crate::pipeline::{
    ConfigRefs, ParentSessionSource, SessionCreationMode,
    execute_with_session_and_meta_with_parent_source,
};
use crate::run_resource_overrides::RunResourceOverrides;
use crate::startup_env::StartupSubtreeEnv;

use super::plan_cmd_child_diagnostics::append_child_diagnostics;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

pub(super) struct StepExecutionOutcome {
    pub(super) exit_code: i32,
    pub(super) output: String,
    pub(super) session_id: Option<String>,
    pub(super) stderr: String,
}

pub(super) struct CsaStepExecutionOptions<'a> {
    pub(super) model_spec: Option<&'a str>,
    pub(super) forwarded_session: Option<&'a str>,
    pub(super) no_fs_sandbox: bool,
    pub(super) readonly_project_root: bool,
    pub(super) resources: RunResourceOverrides,
    pub(super) startup_env: &'a StartupSubtreeEnv,
}

pub(super) async fn run_with_heartbeat<F, T>(
    label: &str,
    execution: F,
    step_started_at: Instant,
) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    let mut execution = std::pin::pin!(execution);
    let mut ticker = tokio::time::interval(HEARTBEAT_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;

    loop {
        tokio::select! {
            result = &mut execution => {
                return result;
            }
            _ = ticker.tick() => {
                eprintln!(
                    "{label} - RUNNING ({:.0}s elapsed)",
                    step_started_at.elapsed().as_secs_f64()
                );
            }
        }
    }
}

pub(super) async fn execute_bash_step(
    label: &str,
    prompt: &str,
    env_vars: &HashMap<String, String>,
    project_root: &Path,
    workflow_path: &Path,
    startup_env: &StartupSubtreeEnv,
    resources: RunResourceOverrides,
) -> Result<StepExecutionOutcome> {
    let script = extract_bash_code_block(prompt).unwrap_or(prompt);
    info!("{} - Executing bash: {}", label, truncate(script, 80));
    for key in env_vars.keys() {
        super::validate_variable_name(key)?;
    }

    let output = match spawn_bash(
        script,
        env_vars,
        project_root,
        workflow_path,
        startup_env,
        resources,
    )
    .await
    {
        Ok(output) => output,
        Err(spawn_error) if is_argument_list_too_long(&spawn_error) => {
            let reduced_env = reduce_bash_env_for_spawn(script, env_vars);
            let dropped_vars = env_vars.len().saturating_sub(reduced_env.len());
            let dropped_bytes: usize = env_vars
                .iter()
                .filter(|(key, _)| !reduced_env.contains_key(*key))
                .map(|(_, value)| value.len())
                .sum();
            if dropped_vars == 0 {
                return Err(spawn_error).context(
                    "Failed to spawn bash (E2BIG) and no reducible STEP_<id>_OUTPUT/SESSION vars",
                );
            }
            warn!(
                "{} - bash spawn hit E2BIG; retrying with reduced STEP_* env (dropped {} vars / {} bytes)",
                label, dropped_vars, dropped_bytes
            );
            spawn_bash(
                script,
                &reduced_env,
                project_root,
                workflow_path,
                startup_env,
                resources,
            )
            .await
            .context("Failed to spawn bash after reducing STEP_* environment")?
        }
        Err(spawn_error) => return Err(spawn_error).context("Failed to spawn bash"),
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let mut stderr_str = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.code().unwrap_or(1) != 0 {
        append_bash_child_diagnostics(&mut stderr_str, project_root, &stdout);
    }
    if !stdout.is_empty() {
        eprint!("{stdout}");
    }
    if !stderr_str.is_empty() {
        eprint!("{stderr_str}");
    }
    Ok(StepExecutionOutcome {
        exit_code: output.status.code().unwrap_or(1),
        output: stdout,
        session_id: None,
        stderr: stderr_str,
    })
}

fn append_bash_child_diagnostics(stderr: &mut String, project_root: &Path, stdout: &str) {
    let original_stderr = stderr.clone();
    let before_len = stderr.len();
    append_child_diagnostics(stderr, project_root, stdout, &original_stderr);
    if stderr.len() > before_len && !stderr.ends_with('\n') {
        stderr.push('\n');
    }
}

async fn spawn_bash(
    script: &str,
    env_vars: &HashMap<String, String>,
    project_root: &Path,
    workflow_path: &Path,
    startup_env: &StartupSubtreeEnv,
    resources: RunResourceOverrides,
) -> std::io::Result<std::process::Output> {
    let workflow_dir = workflow_path.parent().unwrap_or(project_root);
    let current_exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("csa"));
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-c")
        .arg(script)
        .envs(env_vars.iter())
        .env("CSA_PROJECT_ROOT", project_root)
        .env("CSA_WORKFLOW_PATH", workflow_path)
        .env("CSA_WORKFLOW_DIR", workflow_dir)
        .env("CSA_BIN", &current_exe);
    prepend_current_exe_dir_to_path(&mut cmd, env_vars, &current_exe);
    // This bash step is a CSA-child boundary because it may run nested `csa`
    // commands. Reserve the protected contract keys before re-applying CSA's
    // trusted startup snapshot, so workflow/user env cannot spoof session
    // genealogy or subtree pins. A bash step has no session state/sidecar of its
    // own, so nested plan runs that reuse an inherited session contract must not
    // advance CSA_DEPTH beyond the depth that contract can validate.
    csa_core::env::scrub_subtree_contract_env_tokio(&mut cmd);
    cmd.env("CSA_PROJECT_ROOT", project_root)
        .env("CSA_DEPTH", bash_step_depth_string(startup_env))
        .env("CSA_INTERNAL_INVOCATION", "1")
        // Every bash step of a weave pattern is pattern-internal: mark it so any
        // `csa run`/`review`/`debate` it invokes (and their nested CSA children)
        // default the fatal-error-marker scan OFF and cannot self-kill the
        // pipeline on codex-fallback provider-error text (#1847).
        .env(csa_core::env::CSA_PATTERN_INTERNAL_ENV_KEY, "1");
    apply_startup_child_contract_env(&mut cmd, startup_env);
    cmd.env_remove(crate::run_resource_overrides::INHERITED_RESOURCE_OVERRIDES_ENV);
    if let Some(value) = resources.child_env_value().map_err(std::io::Error::other)? {
        cmd.env(
            crate::run_resource_overrides::INHERITED_RESOURCE_OVERRIDES_ENV,
            value,
        );
    }
    cmd.current_dir(project_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
}

fn prepend_current_exe_dir_to_path(
    cmd: &mut tokio::process::Command,
    env_vars: &HashMap<String, String>,
    current_exe: &Path,
) {
    let Some(current_exe_dir) = current_exe_dir_for_path_prepend(current_exe) else {
        return;
    };
    let inherited_path = env_vars
        .get("PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"));
    let mut path_entries = vec![current_exe_dir.to_path_buf()];
    if let Some(path) = inherited_path {
        path_entries.extend(
            std::env::split_paths(&path).filter(|entry| entry.as_path() != current_exe_dir),
        );
    }
    if let Ok(joined_path) = std::env::join_paths(path_entries) {
        cmd.env("PATH", joined_path);
    }
}

fn current_exe_dir_for_path_prepend(current_exe: &Path) -> Option<&Path> {
    let current_exe_dir = current_exe.parent()?;
    if current_exe_dir.as_os_str().is_empty() {
        return None;
    }
    Some(current_exe_dir)
}

fn bash_step_depth_string(startup_env: &StartupSubtreeEnv) -> String {
    // Root/foreground plan sessions may have no startup depth yet; keep the
    // historical child marker there. Once a real parent depth exists, preserve
    // it because the session sidecar/state contract is still the same session.
    if startup_env.current_depth() > 0 {
        return startup_env.current_depth().to_string();
    }

    startup_env.next_depth_string()
}

fn apply_startup_child_contract_env(
    cmd: &mut tokio::process::Command,
    startup_env: &StartupSubtreeEnv,
) {
    for key in StartupSubtreeEnv::csa_child_contract_env_keys() {
        cmd.env_remove(key);
    }
    for (key, value) in startup_env.to_csa_child_contract_env_vars() {
        cmd.env(key, value);
    }
}

fn reduce_bash_env_for_spawn(
    script: &str,
    env_vars: &HashMap<String, String>,
) -> HashMap<String, String> {
    env_vars
        .iter()
        .filter(|(key, _)| {
            let key = key.as_str();
            !is_step_runtime_var(key) || script.contains(key)
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn is_step_runtime_var(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("STEP_") else {
        return false;
    };
    let Some((step_id, suffix)) = rest.split_once('_') else {
        return false;
    };
    !step_id.is_empty()
        && step_id.chars().all(|ch| ch.is_ascii_digit())
        && matches!(suffix, "OUTPUT" | "SESSION")
}

fn is_argument_list_too_long(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::E2BIG)
}

/// Execute a step via CSA tool (codex, claude-code, opencode).
///
/// Stale forwarded session strategy: fallback to a fresh session (approach B).
/// Rationale: token reuse is an optimization, while workflow completion is
/// mandatory for long-running automation. Fallback is never silent: we emit a
/// warning whenever a stale session fallback occurs.
pub(super) struct CsaStepExecutionRequest<'a> {
    pub(super) label: &'a str,
    pub(super) prompt: &'a str,
    pub(super) tool_name: &'a ToolName,
    pub(super) project_root: &'a Path,
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) global_config: &'a csa_config::GlobalConfig,
    pub(super) model_catalog: &'a csa_config::EffectiveModelCatalog,
}

pub(super) async fn execute_csa_step(
    request: CsaStepExecutionRequest<'_>,
    options: CsaStepExecutionOptions<'_>,
) -> Result<StepExecutionOutcome> {
    let CsaStepExecutionRequest {
        label,
        prompt,
        tool_name,
        project_root,
        config,
        global_config,
        model_catalog,
    } = request;
    info!("{} - Dispatching to {} ...", label, tool_name.as_str());

    let executor = crate::pipeline::build_and_validate_executor(
        tool_name,
        options.model_spec,
        None,
        None,
        ConfigRefs {
            project: config,
            global: Some(global_config),
            model_catalog: Some(model_catalog),
        },
        false,
        false,
        false,
    )
    .await?;

    let extra_env = global_config.build_execution_env(
        executor.tool_name(),
        csa_config::ExecutionEnvOptions::default(),
    );
    // #1741: a plan step uses its own per-step tool/model and does NOT consume
    // the parent's subtree pin for that choice, but it MUST still cascade an
    // inherited pin so nested CSA calls from the step stay pinned. The pin is
    // carried out-of-band as a typed value (None unless this process is a pinned
    // child) and applied by the executor's trusted channel — never via the env
    // map.
    let inherited_model_pin =
        crate::run_cmd_model_pin::inherited_model_pin_from_startup(options.startup_env);
    let subtree_pin =
        crate::run_cmd_model_pin::inherited_subtree_model_pin(inherited_model_pin.as_ref());
    let idle_timeout_seconds = crate::pipeline::resolve_idle_timeout_seconds(config, None);
    let initial_response_timeout_seconds =
        crate::pipeline::resolve_initial_response_timeout_for_tool(
            config,
            None,
            None,
            executor.tool_name(),
        );

    let max_concurrent = global_config.max_concurrent(executor.tool_name());
    let slots_dir = csa_config::GlobalConfig::slots_dir()?;
    let _slot_guard = match csa_lock::slot::try_acquire_slot(
        &slots_dir,
        executor.tool_name(),
        max_concurrent,
        None,
    ) {
        Ok(csa_lock::slot::SlotAcquireResult::Acquired(slot)) => slot,
        Ok(csa_lock::slot::SlotAcquireResult::Exhausted(status)) => {
            bail!(
                "All {} slots for '{}' occupied ({}/{})",
                max_concurrent,
                executor.tool_name(),
                status.occupied,
                status.max_slots,
            );
        }
        Err(e) => bail!(
            "Slot acquisition failed for '{}': {}",
            executor.tool_name(),
            e
        ),
    };

    let session_arg = options
        .forwarded_session
        .map(str::trim)
        .filter(|session| !session.is_empty())
        .map(str::to_string);
    let parent_session_id = options.startup_env.session_id().map(ToOwned::to_owned);
    let execute_once = |session_arg: Option<String>| {
        execute_with_session_and_meta_with_parent_source(
            &executor,
            tool_name,
            prompt,
            OutputFormat::Json,
            session_arg,
            false,
            Some("plan-step".to_string()),
            parent_session_id.clone(),
            project_root,
            config,
            extra_env.as_ref(),
            subtree_pin.as_ref(),
            false,
            Some("plan"),
            None,
            None,
            csa_process::StreamMode::TeeToStderr,
            idle_timeout_seconds,
            initial_response_timeout_seconds,
            None,
            None,
            None,
            None,
            ParentSessionSource::ExplicitOnly,
            SessionCreationMode::FreshChild,
            options.resources,
            options.no_fs_sandbox,
            false,
            options.readonly_project_root,
            &[],   // extra_writable
            &[],   // extra_readable
            None, // error_marker_scan_override: plan tool step has no CLI flag; defer to marker/config (#1745/#1847)
            false, // cli_no_hook_bypass_scan: plan has no CLI flag; defer to config
            options.startup_env,
        )
    };
    let result = match execute_once(session_arg.clone()).await {
        Ok(result) => result,
        Err(error) if session_arg.is_some() && is_stale_session_error(&error) => {
            warn!(
                "{} - Forwarded session '{}' is stale; falling back to a new session",
                label,
                session_arg.as_deref().unwrap_or_default(),
            );
            execute_once(None).await?
        }
        Err(error) => return Err(error),
    };

    let raw_captured = if !result.execution.output.is_empty() {
        result.execution.output
    } else if !result.execution.summary.is_empty() {
        result.execution.summary
    } else {
        String::new()
    };
    let captured = clean_step_output_for_env(&raw_captured, tool_name, OutputFormat::Json);
    Ok(StepExecutionOutcome {
        exit_code: result.execution.exit_code,
        output: captured,
        session_id: Some(result.meta_session_id),
        stderr: result.execution.stderr_output,
    })
}

fn clean_step_output_for_env(
    raw_output: &str,
    tool_name: &ToolName,
    output_format: OutputFormat,
) -> String {
    if !should_extract_codex_json_events(raw_output, tool_name, output_format) {
        return raw_output.to_string();
    }

    extract_codex_json_event_text(raw_output).unwrap_or_else(|| raw_output.to_string())
}

fn should_extract_codex_json_events(
    raw_output: &str,
    tool_name: &ToolName,
    output_format: OutputFormat,
) -> bool {
    (matches!(tool_name, ToolName::Codex) && matches!(output_format, OutputFormat::Json))
        || first_non_empty_line_is_thread_started(raw_output)
}

pub(super) fn is_stale_session_error(error: &anyhow::Error) -> bool {
    if error.chain().any(|cause| {
        cause
            .downcast_ref::<csa_core::error::AppError>()
            .is_some_and(|app_error| {
                matches!(
                    app_error,
                    csa_core::error::AppError::SessionNotFound(_)
                        | csa_core::error::AppError::InvalidSessionId(_)
                )
            })
    }) {
        return true;
    }

    error.chain().any(|cause| {
        let msg = cause.to_string();
        msg.contains("No session matching prefix") || msg.contains("Invalid session ID")
    })
}

pub(super) fn extract_bash_code_block(prompt: &str) -> Option<&str> {
    let mut search_start = 0;
    while let Some(fence_offset) = prompt[search_start..].find("```") {
        let fence_start = search_start + fence_offset;
        let line_start = prompt[..fence_start]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        if !prompt[line_start..fence_start]
            .chars()
            .all(char::is_whitespace)
        {
            search_start = fence_start + "```".len();
            continue;
        }

        let open_line_end_offset = prompt[fence_start..].find('\n')?;
        let open_line_end = fence_start + open_line_end_offset;
        let language = prompt[fence_start + "```".len()..open_line_end].trim();
        if !matches!(language, "" | "bash" | "sh") {
            search_start = open_line_end + 1;
            continue;
        }

        let code_start = open_line_end + 1;
        let mut line_start = code_start;
        for line in prompt[code_start..].split_inclusive('\n') {
            let line_end = line_start + line.len();
            if line.trim_end_matches('\n').trim() == "```" {
                let code = &prompt[code_start..line_start];
                return Some(code.trim());
            }
            line_start = line_end;
        }

        return None;
    }
    None
}

pub(super) fn truncate(s: &str, max_len: usize) -> String {
    let first_line = s.lines().next().unwrap_or(s);
    if first_line.len() > max_len {
        format!("{}...", &first_line[..max_len])
    } else {
        first_line.to_string()
    }
}

#[cfg(test)]
#[path = "plan_cmd_exec_tests.rs"]
mod tests;
