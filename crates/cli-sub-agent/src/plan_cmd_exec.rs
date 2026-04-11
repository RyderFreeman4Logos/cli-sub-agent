use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tracing::{info, warn};

use csa_config::ProjectConfig;
use csa_core::types::{OutputFormat, ToolName};
use csa_process::check_tool_installed;

use crate::pipeline::execute_with_session_and_meta;
use crate::run_helpers::build_executor;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

pub(super) struct StepExecutionOutcome {
    pub(super) exit_code: i32,
    pub(super) output: String,
    pub(super) session_id: Option<String>,
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
) -> Result<StepExecutionOutcome> {
    let script = extract_bash_code_block(prompt).unwrap_or(prompt);
    info!("{} - Executing bash: {}", label, truncate(script, 80));
    for key in env_vars.keys() {
        super::validate_variable_name(key)?;
    }

    let output = match spawn_bash(script, env_vars, project_root, workflow_path).await {
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
            spawn_bash(script, &reduced_env, project_root, workflow_path)
                .await
                .context("Failed to spawn bash after reducing STEP_* environment")?
        }
        Err(spawn_error) => return Err(spawn_error).context("Failed to spawn bash"),
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !stdout.is_empty() {
        eprint!("{stdout}");
    }
    Ok(StepExecutionOutcome {
        exit_code: output.status.code().unwrap_or(1),
        output: stdout,
        session_id: None,
    })
}

fn next_csa_depth() -> String {
    let current_depth = std::env::var("CSA_DEPTH")
        .ok()
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or(0);
    current_depth.saturating_add(1).to_string()
}

async fn spawn_bash(
    script: &str,
    env_vars: &HashMap<String, String>,
    project_root: &Path,
    workflow_path: &Path,
) -> std::io::Result<std::process::Output> {
    let workflow_dir = workflow_path.parent().unwrap_or(project_root);
    tokio::process::Command::new("bash")
        .arg("-c")
        .arg(script)
        .envs(env_vars.iter())
        .env("CSA_PROJECT_ROOT", project_root)
        .env("CSA_WORKFLOW_PATH", workflow_path)
        .env("CSA_WORKFLOW_DIR", workflow_dir)
        .env("CSA_DEPTH", next_csa_depth())
        .env("CSA_INTERNAL_INVOCATION", "1")
        .current_dir(project_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .output()
        .await
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
    model_spec: Option<&str>,
    forwarded_session: Option<&str>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
) -> Result<StepExecutionOutcome> {
    info!("{} - Dispatching to {} ...", label, tool_name.as_str());

    let executor = build_executor(tool_name, model_spec, None, None, config, false)?;
    check_tool_installed(executor.runtime_binary_name()).await?;

    let global_config = csa_config::GlobalConfig::load()?;
    let extra_env = global_config.build_execution_env(
        executor.tool_name(),
        csa_config::ExecutionEnvOptions::default(),
    );
    let idle_timeout_seconds = crate::pipeline::resolve_idle_timeout_seconds(config, None);
    let initial_response_timeout_seconds =
        crate::pipeline::resolve_initial_response_timeout_seconds(config, None);

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

    let session_arg = forwarded_session
        .map(str::trim)
        .filter(|session| !session.is_empty())
        .map(str::to_string);
    let execute_once = |session_arg: Option<String>| {
        execute_with_session_and_meta(
            &executor,
            tool_name,
            prompt,
            OutputFormat::Json,
            session_arg,
            Some("plan-step".to_string()),
            std::env::var("CSA_SESSION_ID").ok(),
            project_root,
            config,
            extra_env.as_ref(),
            Some("plan"),
            None,
            None,
            csa_process::StreamMode::TeeToStderr,
            idle_timeout_seconds,
            initial_response_timeout_seconds,
            None,
            None,
            None,
            false, // no_fs_sandbox
            false, // readonly_project_root
            &[],   // extra_writable
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

    let captured = if !result.execution.output.is_empty() {
        result.execution.output
    } else if !result.execution.summary.is_empty() {
        result.execution.summary
    } else {
        String::new()
    };
    Ok(StepExecutionOutcome {
        exit_code: result.execution.exit_code,
        output: captured,
        session_id: Some(result.meta_session_id),
    })
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
mod tests {
    use super::*;
    use crate::test_env_lock::TEST_ENV_LOCK;

    #[test]
    fn is_step_runtime_var_only_matches_step_output_and_session() {
        assert!(is_step_runtime_var("STEP_1_OUTPUT"));
        assert!(is_step_runtime_var("STEP_22_SESSION"));
        assert!(!is_step_runtime_var("STEP_OUTPUT"));
        assert!(!is_step_runtime_var("STEP_1_OUTPUT_JSON"));
        assert!(!is_step_runtime_var("STEP_A_OUTPUT"));
        assert!(!is_step_runtime_var("USER_LANGUAGE"));
    }

    #[test]
    fn reduce_bash_env_for_spawn_drops_unreferenced_step_runtime_vars() {
        let env_vars = HashMap::from([
            ("STEP_1_OUTPUT".to_string(), "large".to_string()),
            ("STEP_2_SESSION".to_string(), "sid".to_string()),
            (
                "USER_LANGUAGE".to_string(),
                "Chinese (Simplified)".to_string(),
            ),
        ]);

        let reduced = reduce_bash_env_for_spawn("echo ok", &env_vars);
        assert!(!reduced.contains_key("STEP_1_OUTPUT"));
        assert!(!reduced.contains_key("STEP_2_SESSION"));
        assert_eq!(
            reduced.get("USER_LANGUAGE").map(String::as_str),
            Some("Chinese (Simplified)")
        );
    }

    #[test]
    fn reduce_bash_env_for_spawn_keeps_referenced_step_runtime_vars() {
        let env_vars = HashMap::from([
            ("STEP_1_OUTPUT".to_string(), "payload".to_string()),
            ("STEP_2_SESSION".to_string(), "sid".to_string()),
            ("SCOPE".to_string(), "demo".to_string()),
        ]);

        let script = "printf '%s' \"${STEP_1_OUTPUT}\"; printenv STEP_2_SESSION >/dev/null";
        let reduced = reduce_bash_env_for_spawn(script, &env_vars);
        assert_eq!(
            reduced.get("STEP_1_OUTPUT").map(String::as_str),
            Some("payload")
        );
        assert_eq!(
            reduced.get("STEP_2_SESSION").map(String::as_str),
            Some("sid")
        );
        assert_eq!(reduced.get("SCOPE").map(String::as_str), Some("demo"));
    }

    #[test]
    fn next_csa_depth_increments_or_defaults() {
        let _env_lock = TEST_ENV_LOCK.lock().expect("plan env lock poisoned");
        let original_depth = std::env::var("CSA_DEPTH").ok();

        // SAFETY: test-scoped env mutation.
        unsafe {
            std::env::remove_var("CSA_DEPTH");
        }
        assert_eq!(next_csa_depth(), "1");

        // SAFETY: test-scoped env mutation.
        unsafe {
            std::env::set_var("CSA_DEPTH", "2");
        }
        assert_eq!(next_csa_depth(), "3");

        // SAFETY: restore original env value.
        unsafe {
            match original_depth {
                Some(value) => std::env::set_var("CSA_DEPTH", value),
                None => std::env::remove_var("CSA_DEPTH"),
            }
        }
    }
}
