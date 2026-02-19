//! Transport execution helpers for pipeline, including SIGTERM-safe interruption handling.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use tracing::warn;

use csa_executor::{ExecuteOptions, Executor, SessionConfig, TransportResult};
use csa_session::{MetaSessionState, SessionResult, ToolState, save_result, save_session};

use crate::session_guard::{SessionCleanupGuard, write_pre_exec_error_result};

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
) -> Result<TransportResult> {
    let exec_result = {
        #[cfg(unix)]
        {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .context("Failed to register SIGTERM handler")?;
            tokio::select! {
                _ = sigterm.recv() => {
                    let interrupted_at = chrono::Utc::now();
                    let interrupted_result = SessionResult {
                        status: "interrupted".to_string(),
                        exit_code: 143,
                        summary: "Execution interrupted by SIGTERM".to_string(),
                        tool: executor.tool_name().to_string(),
                        started_at: execution_start_time,
                        completed_at: interrupted_at,
                        artifacts: Vec::new(),
                    };
                    if let Err(e) = save_result(project_root, &session.meta_session_id, &interrupted_result) {
                        warn!("Failed to save interrupted session result: {}", e);
                    }
                    if let Err(e) = save_session(session) {
                        warn!("Failed to save session state after SIGTERM: {}", e);
                    }
                    if let Some(cg) = cleanup_guard {
                        cg.defuse();
                    }
                    return Err(anyhow::anyhow!("Execution interrupted by SIGTERM"));
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
    };

    match exec_result {
        Ok(result) => Ok(result),
        Err(e) => {
            write_pre_exec_error_result(
                project_root,
                &session.meta_session_id,
                executor.tool_name(),
                &e,
            );
            if let Some(cg) = cleanup_guard {
                cg.defuse();
            }
            Err(e).context("Failed to execute tool via transport")
        }
    }
}
