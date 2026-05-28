use std::path::Path;

use csa_config::ProjectConfig;
use csa_executor::Executor;
use csa_resource::{ResourceGuard, ResourceLimits};
use csa_session::{MetaSessionState, save_session};
use tracing::warn;

use crate::session_guard::{SessionCleanupGuard, write_pre_exec_error_result};

pub(super) fn check_resources_before_spawn(
    config: Option<&ProjectConfig>,
    executor: &Executor,
    project_root: &Path,
    session: &mut MetaSessionState,
    cleanup_guard: &mut Option<SessionCleanupGuard>,
) -> anyhow::Result<()> {
    let mut resource_guard = config.map(|cfg| {
        ResourceGuard::new(ResourceLimits {
            min_free_memory_mb: cfg.resources.min_free_memory_mb,
        })
    });
    if let Some(ref mut guard) = resource_guard
        && let Err(err) = guard.check_availability(executor.tool_name())
    {
        return Err(persist_pipeline_pre_exec_failure(
            project_root,
            session,
            executor.tool_name(),
            err,
            cleanup_guard,
            Some("low_memory"),
        ));
    }
    if let (Some(guard), Some(cfg)) = (&mut resource_guard, config) {
        guard.check_health(
            cfg.sandbox_memory_max_mb(executor.tool_name()),
            cfg.sandbox_memory_swap_max_mb(executor.tool_name()),
            60,
        );
    }
    Ok(())
}

pub(super) fn write_fatal_error_marker_sidecar(
    config: Option<&ProjectConfig>,
    session_dir: &Path,
    project_root: &Path,
    session: &mut MetaSessionState,
    tool_name: &str,
    cleanup_guard: &mut Option<SessionCleanupGuard>,
) -> anyhow::Result<()> {
    let Some(cfg) = config else {
        return Ok(());
    };
    csa_process::write_fatal_error_markers(session_dir, &cfg.resources.fatal_error_markers).map_err(
        |err| {
            persist_pipeline_pre_exec_failure(
                project_root,
                session,
                tool_name,
                anyhow::anyhow!(err).context("Failed to write fatal error marker sidecar"),
                cleanup_guard,
                None,
            )
        },
    )
}

pub(super) fn persist_pipeline_pre_exec_failure(
    project_root: &Path,
    session: &mut MetaSessionState,
    tool_name: &str,
    err: anyhow::Error,
    cleanup_guard: &mut Option<SessionCleanupGuard>,
    termination_reason: Option<&str>,
) -> anyhow::Error {
    write_pre_exec_error_result(project_root, &session.meta_session_id, tool_name, &err);
    if let Some(reason) = termination_reason {
        session.termination_reason = Some(reason.to_string());
        session.last_accessed = chrono::Utc::now();
        if let Err(save_err) = save_session(session) {
            warn!(
                session = %session.meta_session_id,
                error = %save_err,
                termination_reason = reason,
                "Failed to persist pre-exec termination reason"
            );
        }
    }
    if let Some(cg) = cleanup_guard {
        cg.defuse();
    }
    err
}
