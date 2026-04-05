//! Transport execution helpers for pipeline, including signal-safe interruption handling.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tracing::warn;

use csa_executor::{ExecuteOptions, Executor, PeakMemoryContext, SessionConfig, TransportResult};
use csa_session::{MetaSessionState, SessionResult, ToolState, save_result, save_session};

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
        Ok(result) => Ok(result),
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
            let result = SessionResult {
                status: "failure".to_string(),
                exit_code: 1,
                summary: format!("transport: {e}"),
                tool: executor.tool_name().to_string(),
                started_at: execution_start_time,
                completed_at,
                events_count: 0,
                artifacts: Vec::new(),
                peak_memory_mb,
            };
            if let Err(save_err) = save_result(project_root, &session.meta_session_id, &result) {
                warn!("Failed to save transport error result: {}", save_err);
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
        started_at: execution_start_time,
        completed_at,
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
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
