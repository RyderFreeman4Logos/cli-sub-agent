use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tracing::{info, warn};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_process::check_tool_installed;

use crate::pipeline::execute_with_session_and_meta;
use crate::run_helpers::build_executor;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);

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
) -> Result<StepExecutionOutcome> {
    let script = extract_bash_code_block(prompt).unwrap_or(prompt);
    info!("{} - Executing bash: {}", label, truncate(script, 80));
    for key in env_vars.keys() {
        super::validate_variable_name(key)?;
    }

    let output = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(script)
        .envs(env_vars.iter())
        .current_dir(project_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .output()
        .await
        .context("Failed to spawn bash")?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !stdout.is_empty() {
        eprint!("{}", stdout);
    }
    Ok(StepExecutionOutcome {
        exit_code: output.status.code().unwrap_or(1),
        output: stdout,
        session_id: None,
    })
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

    let executor = build_executor(tool_name, model_spec, None, None, config)?;
    check_tool_installed(executor.runtime_binary_name()).await?;

    let global_config = csa_config::GlobalConfig::load()?;
    let extra_env = global_config.env_vars(executor.tool_name()).cloned();
    let idle_timeout_seconds = crate::pipeline::resolve_idle_timeout_seconds(config, None);

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
            None,
            None,
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
