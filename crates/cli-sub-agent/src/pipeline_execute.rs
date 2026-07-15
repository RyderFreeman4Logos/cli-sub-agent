//! Transport execution helpers for pipeline, including signal-safe interruption handling.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tracing::warn;

use csa_core::transport_events::StreamingMetadata;
use csa_executor::command_isolation::{CleanCommandContract, CommandIsolationPolicy};
use csa_executor::{ExecuteOptions, Executor, PeakMemoryContext, SessionConfig, TransportResult};
use csa_session::{
    MetaSessionState, PhaseEvent, SessionPhase, SessionResult, ToolState, save_result, save_session,
};

use crate::session_guard::SessionCleanupGuard;

const RUN_TIMEOUT_EXIT_CODE: i32 = 124;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportFailurePolicy {
    Legacy,
    CleanRoom,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_transport_with_signal(
    executor: &Executor,
    effective_prompt: &str,
    tool_state: Option<&ToolState>,
    session: &MetaSessionState,
    merged_env_ref: Option<&HashMap<String, String>>,
    execute_options: ExecuteOptions,
    session_config: Option<SessionConfig>,
    project_root: &Path,
    cleanup_guard: &mut Option<SessionCleanupGuard>,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    wall_timeout: Option<Duration>,
) -> Result<TransportResult> {
    let failure_policy = TransportFailurePolicy::Legacy;
    let exec_result = {
        #[cfg(unix)]
        {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .context("Failed to register SIGTERM handler")?;
            let mut sigint =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                    .context("Failed to register SIGINT handler")?;
            let timeout_future = async {
                if let Some(timeout) = wall_timeout {
                    tokio::time::sleep(timeout).await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            tokio::pin!(timeout_future);
            tokio::select! {
                _ = sigterm.recv() => {
                    warn!(
                        session_id = %session.meta_session_id,
                        task_type = session.task_context.task_type.as_deref().unwrap_or("unknown"),
                        tool = %executor.tool_name(),
                        wall_timeout_secs = ?wall_timeout.map(|timeout| timeout.as_secs()),
                        "Session received SIGTERM; classifying as external signal, not CSA idle or wall-clock timeout"
                    );
                    record_session_interruption_state(
                        project_root,
                        session,
                        "sigterm",
                        cleanup_guard,
                    );
                    return Ok(signal_interrupted_transport_result(
                        143,
                        Some(libc::SIGTERM),
                        "sigterm",
                        "Execution interrupted by SIGTERM",
                    ));
                }
                _ = sigint.recv() => {
                    record_session_interruption_state(
                        project_root,
                        session,
                        "sigint",
                        cleanup_guard,
                    );
                    return Ok(signal_interrupted_transport_result(
                        130,
                        Some(libc::SIGINT),
                        "sigint",
                        "Execution interrupted by SIGINT",
                    ));
                }
                _ = &mut timeout_future => {
                    let timeout_secs = wall_timeout.map_or(1, |timeout| timeout.as_secs().max(1));
                    let summary = format!("Execution timed out after {timeout_secs}s");
                    warn!(
                        session_id = %session.meta_session_id,
                        task_type = session.task_context.task_type.as_deref().unwrap_or("unknown"),
                        tool = %executor.tool_name(),
                        timeout_secs,
                        "Session wall-clock timeout fired"
                    );
                    record_session_interruption_state(
                        project_root,
                        session,
                        "timeout",
                        cleanup_guard,
                    );
                    return Ok(signal_interrupted_transport_result(
                        RUN_TIMEOUT_EXIT_CODE,
                        None,
                        "timeout",
                        &summary,
                    ));
                }
                exec = executor.execute_with_transport(
                    effective_prompt,
                    tool_state,
                    session,
                    merged_env_ref,
                    execute_options,
                    session_config,
                ) => exec,
            }
        }
        #[cfg(not(unix))]
        {
            if let Some(timeout) = wall_timeout {
                match tokio::time::timeout(
                    timeout,
                    executor.execute_with_transport(
                        effective_prompt,
                        tool_state,
                        session,
                        merged_env_ref,
                        execute_options,
                        session_config,
                    ),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        let timeout_secs = timeout.as_secs().max(1);
                        let summary = format!("Execution timed out after {timeout_secs}s");
                        record_session_interruption_state(
                            project_root,
                            session,
                            "timeout",
                            cleanup_guard,
                        );
                        return Ok(signal_interrupted_transport_result(
                            RUN_TIMEOUT_EXIT_CODE,
                            None,
                            "timeout",
                            &summary,
                        ));
                    }
                }
            } else {
                executor
                    .execute_with_transport(
                        effective_prompt,
                        tool_state,
                        session,
                        merged_env_ref,
                        execute_options,
                        session_config,
                    )
                    .await
            }
        }
    };

    match exec_result {
        Ok(result) => {
            log_signal_exit_diagnostic(session, &result.execution);
            Ok(result)
        }
        Err(e) => {
            debug_assert_eq!(failure_policy, TransportFailurePolicy::Legacy);
            // Record a failure result with accurate timing: started_at is when
            // execution began (before ACP init), not "now", fixing the
            // Start == End timing bug for slow failures like ACP init timeout.
            let completed_at = chrono::Utc::now();
            // Extract peak_memory_mb from error context if the sandbox
            // captured it before the ACP session failed.
            let peak_memory_mb = e
                .chain()
                .find_map(|cause| cause.downcast_ref::<PeakMemoryContext>())
                .and_then(|ctx| ctx.0);
            let error_summary = format!("transport: {e}");
            // The anyhow error chain is the provider/transport error channel for
            // a failed turn (the agent's reviewed stdout is discarded on Err), so
            // it is the correct — and only — source for a permanent quota verdict
            // (#1736).
            let exhaustion = crate::run_cmd_post::detect_permanent_tool_exhaustion_text(
                executor.tool_name(),
                &format!("{e:#}"),
                1,
                None,
            );
            let summary = exhaustion.as_ref().map_or(error_summary, |detected| {
                crate::run_cmd_post::format_tool_exhausted_summary(
                    executor.tool_name(),
                    &detected.matched_pattern,
                )
            });
            let result = SessionResult {
                post_exec_gate: None,
                status: "failure".to_string(),
                exit_code: 1,
                summary: summary.clone(),
                tool: executor.tool_name().to_string(),
                original_tool: None,
                fallback_tool: None,
                fallback_reason: None,
                started_at: execution_start_time,
                completed_at,
                events_count: 0,
                artifacts: Vec::new(),
                peak_memory_mb,
                kill_hint: None,
                last_item: None,
                fallback_chain: None,
                ..Default::default()
            };
            if let Err(save_err) = save_result(project_root, &session.meta_session_id, &result) {
                warn!("Failed to save transport error result: {}", save_err);
            }
            if exhaustion.is_some() {
                let mut updated_session = session.clone();
                if let Err(err) = updated_session.apply_phase_event(PhaseEvent::ToolExhausted) {
                    warn!(
                        session = %updated_session.meta_session_id,
                        error = %err,
                        "Skipping phase transition on transport tool exhaustion"
                    );
                    updated_session.phase = SessionPhase::ToolExhausted;
                }
                updated_session.termination_reason = Some("tool_exhausted".to_string());
                updated_session.last_accessed = completed_at;
                if let Some(tool_state) = updated_session.tools.get_mut(executor.tool_name()) {
                    tool_state.last_exit_code = 1;
                    tool_state.last_action_summary = summary;
                    tool_state.updated_at = completed_at;
                }
                if let Err(save_err) = save_session(&updated_session) {
                    warn!(
                        "Failed to save transport tool exhaustion state: {}",
                        save_err
                    );
                }
            }
            // Best-effort cooldown marker
            csa_session::write_cooldown_marker_for_project(
                project_root,
                &session.meta_session_id,
                completed_at,
            );
            if let Some(cg) = cleanup_guard {
                cg.defuse();
            }
            Err(e).context("Failed to execute tool via transport")
        }
    }
}

pub(crate) async fn execute_clean_transport_with_signal(
    executor: &Executor,
    effective_prompt: &str,
    session: &MetaSessionState,
    execute_options: ExecuteOptions,
    command: CleanCommandContract,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    wall_timeout: Option<Duration>,
) -> Result<TransportResult> {
    let failure_policy = TransportFailurePolicy::CleanRoom;
    let exec_result = {
        #[cfg(unix)]
        {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .context("Failed to register clean-room SIGTERM handler")?;
            let mut sigint =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                    .context("Failed to register clean-room SIGINT handler")?;
            let timeout_future = async {
                if let Some(timeout) = wall_timeout {
                    tokio::time::sleep(timeout).await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            tokio::pin!(timeout_future);
            tokio::select! {
                _ = sigterm.recv() => Ok(signal_interrupted_transport_result(
                    143,
                    Some(libc::SIGTERM),
                    "sigterm",
                    "Execution interrupted by SIGTERM",
                )),
                _ = sigint.recv() => Ok(signal_interrupted_transport_result(
                    130,
                    Some(libc::SIGINT),
                    "sigint",
                    "Execution interrupted by SIGINT",
                )),
                _ = &mut timeout_future => {
                    let timeout_secs = wall_timeout.map_or(1, |timeout| timeout.as_secs().max(1));
                    Ok(signal_interrupted_transport_result(
                        RUN_TIMEOUT_EXIT_CODE,
                        None,
                        "timeout",
                        &format!("Execution timed out after {timeout_secs}s"),
                    ))
                },
                result = executor.execute_with_command_isolation(
                    effective_prompt,
                    None,
                    session,
                    None,
                    execute_options,
                    None,
                    CommandIsolationPolicy::CleanRoom(command),
                ) => result,
            }
        }
        #[cfg(not(unix))]
        {
            let execution = executor.execute_with_command_isolation(
                effective_prompt,
                None,
                session,
                None,
                execute_options,
                None,
                CommandIsolationPolicy::CleanRoom(command),
            );
            if let Some(timeout) = wall_timeout {
                match tokio::time::timeout(timeout, execution).await {
                    Ok(result) => result,
                    Err(_) => Ok(signal_interrupted_transport_result(
                        RUN_TIMEOUT_EXIT_CODE,
                        None,
                        "timeout",
                        &format!("Execution timed out after {}s", timeout.as_secs().max(1)),
                    )),
                }
            } else {
                execution.await
            }
        }
    };
    match exec_result {
        Ok(result) => Ok(result),
        Err(error) => {
            debug_assert_eq!(failure_policy, TransportFailurePolicy::CleanRoom);
            let elapsed = chrono::Utc::now()
                .signed_duration_since(execution_start_time)
                .num_milliseconds();
            Err(error).with_context(|| {
                format!("clean-room transport failed after {elapsed}ms without legacy side effects")
            })
        }
    }
}

fn signal_interrupted_transport_result(
    exit_code: i32,
    exit_signal: Option<i32>,
    terminal_reason: &str,
    summary: &str,
) -> TransportResult {
    TransportResult {
        execution: csa_process::ExecutionResult {
            output: String::new(),
            stderr_output: summary.to_string(),
            summary: summary.to_string(),
            exit_code,
            model_completed: Some(false),
            terminal_reason: Some(terminal_reason.to_string()),
            raw_process_exit_code: Some(exit_code),
            exit_signal,
            ..Default::default()
        },
        provider_session_id: None,
        events: Vec::new(),
        metadata: StreamingMetadata::default(),
    }
}

fn log_signal_exit_diagnostic(
    session: &MetaSessionState,
    execution: &csa_process::ExecutionResult,
) {
    let task_type = session.task_context.task_type.as_deref();
    if !matches!(task_type, Some("review" | "debate")) {
        return;
    }
    if !matches!(execution.exit_code, 124 | 137 | 143) {
        return;
    }

    let diagnosis = diagnose_signal_exit(session, execution);
    warn!(
        session_id = %session.meta_session_id,
        task_type = task_type.unwrap_or("unknown"),
        exit_code = execution.exit_code,
        termination_reason = ?session.termination_reason,
        diagnosis,
        summary = %execution.summary,
        "Review/debate tool process ended with signal-like status"
    );
}

fn diagnose_signal_exit(
    session: &MetaSessionState,
    execution: &csa_process::ExecutionResult,
) -> &'static str {
    let summary = execution.summary.to_ascii_lowercase();
    let stderr = execution.stderr_output.to_ascii_lowercase();
    let haystack = format!("{summary}\n{stderr}");

    if session.termination_reason.as_deref() == Some("timeout") || execution.exit_code == 124 {
        return "wall_clock_timeout";
    }
    if haystack.contains("initial_response_timeout") {
        return "initial_response_timeout";
    }
    if haystack.contains("idle timeout") {
        return "idle_timeout";
    }
    if haystack.contains("oom") || haystack.contains("memory") {
        return "oom_or_memory_limit";
    }
    if execution.exit_code == 143 || session.termination_reason.as_deref() == Some("sigterm") {
        return "external_sigterm";
    }
    if execution.exit_code == 137 {
        return "sigkill_or_oom";
    }
    "external_signal"
}

fn record_session_interruption_state(
    project_root: &Path,
    session: &MetaSessionState,
    termination_reason: &str,
    cleanup_guard: &mut Option<SessionCleanupGuard>,
) {
    let completed_at = chrono::Utc::now();
    let mut updated_session = session.clone();
    updated_session.termination_reason = Some(termination_reason.to_string());
    // Best-effort cooldown marker
    csa_session::write_cooldown_marker_for_project(
        project_root,
        &session.meta_session_id,
        completed_at,
    );
    if let Err(e) = save_session(&updated_session) {
        warn!(
            "Failed to save session state after {}: {}",
            termination_reason, e
        );
    }
    if let Some(cg) = cleanup_guard {
        cg.defuse();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_interrupted_transport_result_models_sigterm_as_incomplete_turn() {
        let result = signal_interrupted_transport_result(
            143,
            Some(libc::SIGTERM),
            "sigterm",
            "Execution interrupted by SIGTERM",
        );

        assert_eq!(result.execution.exit_code, 143);
        assert_eq!(result.execution.raw_process_exit_code, Some(143));
        assert_eq!(result.execution.exit_signal, Some(libc::SIGTERM));
        assert_eq!(result.execution.terminal_reason.as_deref(), Some("sigterm"));
        assert_eq!(result.execution.model_completed, Some(false));
        assert!(result.events.is_empty());
    }
}
