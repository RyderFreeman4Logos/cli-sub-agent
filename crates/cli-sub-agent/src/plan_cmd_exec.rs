use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tracing::{info, warn};

use csa_config::ProjectConfig;
use csa_core::types::{OutputFormat, ToolName};
use csa_process::check_tool_installed;

use crate::codex_transcript_filter::{
    extract_codex_json_event_text, first_non_empty_line_is_thread_started,
};
use crate::pipeline::{
    ParentSessionSource, SessionCreationMode, execute_with_session_and_meta_with_parent_source,
};
use crate::run_helpers::build_executor;
use crate::startup_env::StartupSubtreeEnv;

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
) -> Result<StepExecutionOutcome> {
    let script = extract_bash_code_block(prompt).unwrap_or(prompt);
    info!("{} - Executing bash: {}", label, truncate(script, 80));
    for key in env_vars.keys() {
        super::validate_variable_name(key)?;
    }

    let output = match spawn_bash(script, env_vars, project_root, workflow_path, startup_env).await
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
            )
            .await
            .context("Failed to spawn bash after reducing STEP_* environment")?
        }
        Err(spawn_error) => return Err(spawn_error).context("Failed to spawn bash"),
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr_str = String::from_utf8_lossy(&output.stderr).to_string();
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

async fn spawn_bash(
    script: &str,
    env_vars: &HashMap<String, String>,
    project_root: &Path,
    workflow_path: &Path,
    startup_env: &StartupSubtreeEnv,
) -> std::io::Result<std::process::Output> {
    let workflow_dir = workflow_path.parent().unwrap_or(project_root);
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-c")
        .arg(script)
        .envs(env_vars.iter())
        .env("CSA_PROJECT_ROOT", project_root)
        .env("CSA_WORKFLOW_PATH", workflow_path)
        .env("CSA_WORKFLOW_DIR", workflow_dir)
        .env("CSA_DEPTH", startup_env.next_depth_string())
        .env("CSA_INTERNAL_INVOCATION", "1");
    // #1741: this bash step is marked as a nested CSA invocation (CSA_DEPTH set
    // above), and `bash` inherits the parent's ambient environment. Any ambient
    // SUBTREE_PIN_ENV_KEYS would otherwise be read by a nested `csa run` inside
    // the step as a valid inherited subtree pin, letting a user-controlled
    // ambient env spoof a pin and silently drop tier routing. Reserve the keys
    // (env_remove) BEFORE re-applying CSA's own legitimately-inherited pin via
    // the trusted typed channel — so the keys reach the child IFF CSA decided to
    // pin, never from ambient/user env. (csa-core/src/env.rs reservation.)
    apply_sanitized_subtree_pin(&mut cmd, startup_env);
    cmd.current_dir(project_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
}

/// Reserve the subtree-pin env keys on a child `Command` (which inherits the
/// parent env), then re-apply CSA's own legitimately-inherited pin via the
/// trusted typed channel (#1741).
///
/// Used by non-executor spawn paths that mark their child as a nested CSA
/// invocation (set/propagate `CSA_DEPTH`) AND inherit the parent environment.
/// Without the reservation, an ambient/user-controlled `CSA_MODEL_SPEC` +
/// `CSA_FORCE_IGNORE_TIER_SETTING` pair would be honored as a subtree pin by a
/// nested `csa run`. The typed [`SubtreeModelPin`] re-applied here is the sole
/// writer of the pin keys (and only when this process genuinely inherited a
/// pin), so a legitimately-propagated pin still cascades unbroken.
fn apply_sanitized_subtree_pin(cmd: &mut tokio::process::Command, startup_env: &StartupSubtreeEnv) {
    for key in csa_core::env::SUBTREE_PIN_ENV_KEYS {
        cmd.env_remove(key);
    }
    let inherited_model_pin =
        crate::run_cmd_model_pin::inherited_model_pin_from_startup(startup_env);
    if let Some(pin) =
        crate::run_cmd_model_pin::inherited_subtree_model_pin(inherited_model_pin.as_ref())
    {
        for (key, value) in pin.pin_env_entries() {
            cmd.env(key, value);
        }
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

/// Execute a step via CSA tool (codex, claude-code, gemini-cli, opencode).
///
/// Stale forwarded session strategy: fallback to a fresh session (approach B).
/// Rationale: token reuse is an optimization, while workflow completion is
/// mandatory for long-running automation. Fallback is never silent: we emit a
/// warning whenever a stale session fallback occurs.
pub(super) async fn execute_csa_step(
    label: &str,
    prompt: &str,
    tool_name: &ToolName,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    options: CsaStepExecutionOptions<'_>,
) -> Result<StepExecutionOutcome> {
    info!("{} - Dispatching to {} ...", label, tool_name.as_str());

    let executor = build_executor(tool_name, options.model_spec, None, None, config, false)?;
    check_tool_installed(executor.runtime_binary_name()).await?;

    let global_config = csa_config::GlobalConfig::load()?;
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
            options.no_fs_sandbox,
            options.readonly_project_root,
            &[],   // extra_writable
            &[],   // extra_readable
            false, // cli_no_error_marker_scan: plan has no CLI flag; defer to config (#1745)
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
    let start_patterns = ["```bash\n", "```sh\n", "```\n"];
    for pattern in &start_patterns {
        if let Some(start_idx) = prompt.find(pattern) {
            let code_start = start_idx + pattern.len();
            if let Some(end_idx) = prompt[code_start..].find("```") {
                let code = &prompt[code_start..code_start + end_idx];
                return Some(code.trim());
            }
        }
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
