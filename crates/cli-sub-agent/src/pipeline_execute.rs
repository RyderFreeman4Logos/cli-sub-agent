//! Transport execution helpers for pipeline, including signal-safe interruption handling.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tracing::warn;

use csa_executor::{ExecuteOptions, Executor, PeakMemoryContext, SessionConfig, TransportResult};
use csa_session::{
    MetaSessionState, PhaseEvent, SessionPhase, SessionResult, ToolState, save_result, save_session,
};

use crate::session_guard::SessionCleanupGuard;

const RUN_TIMEOUT_EXIT_CODE: i32 = 124;

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
                    record_session_termination(
                        project_root,
                        session,
                        executor.tool_name(),
                        execution_start_time,
                        "sigterm",
                        "interrupted",
                        143,
                        "Execution interrupted by SIGTERM",
                        cleanup_guard,
                    );
                    return Err(anyhow::anyhow!("Execution interrupted by SIGTERM"));
                }
                _ = sigint.recv() => {
                    record_session_termination(
                        project_root,
                        session,
                        executor.tool_name(),
                        execution_start_time,
                        "sigint",
                        "interrupted",
                        130,
                        "Execution interrupted by SIGINT",
                        cleanup_guard,
                    );
                    return Err(anyhow::anyhow!("Execution interrupted by SIGINT"));
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
                    record_session_termination(
                        project_root,
                        session,
                        executor.tool_name(),
                        execution_start_time,
                        "timeout",
                        "timed_out",
                        RUN_TIMEOUT_EXIT_CODE,
                        &summary,
                        cleanup_guard,
                    );
                    return Err(anyhow::anyhow!(
                        "Execution interrupted by WALL_TIMEOUT timeout_secs={timeout_secs}"
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
                        record_session_termination(
                            project_root,
                            session,
                            executor.tool_name(),
                            execution_start_time,
                            "timeout",
                            "timed_out",
                            RUN_TIMEOUT_EXIT_CODE,
                            &summary,
                            cleanup_guard,
                        );
                        return Err(anyhow::anyhow!(
                            "Execution interrupted by WALL_TIMEOUT timeout_secs={timeout_secs}"
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
            let exhaustion = crate::run_cmd_post::detect_permanent_tool_exhaustion_text(
                executor.tool_name(),
                &error_summary,
                &format!("{e:#}"),
                "",
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
                fallback_chain: None,
                gate_timeout: false,
                manager_fields: Default::default(),
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

#[allow(clippy::too_many_arguments)]
fn record_session_termination(
    project_root: &Path,
    session: &MetaSessionState,
    tool_name: &str,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    termination_reason: &str,
    status: &str,
    exit_code: i32,
    summary: &str,
    cleanup_guard: &mut Option<SessionCleanupGuard>,
) {
    let completed_at = chrono::Utc::now();
    let mut updated_session = session.clone();
    updated_session.termination_reason = Some(termination_reason.to_string());
    let updated_result = SessionResult {
        status: status.to_string(),
        exit_code,
        summary: summary.to_string(),
        tool: tool_name.to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: execution_start_time,
        completed_at,
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        fallback_chain: None,
        gate_timeout: false,
        manager_fields: Default::default(),
    };
    if let Err(e) = save_result(project_root, &session.meta_session_id, &updated_result) {
        warn!(
            "Failed to save session result after {}: {}",
            termination_reason, e
        );
    }
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
