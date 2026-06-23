use std::path::Path;

use csa_config::ProjectConfig;
use csa_executor::Executor;
use csa_resource::{ResourceGuard, ResourceLimits};
use csa_session::{MetaSessionState, save_session};
use tracing::warn;

use crate::resource_admission::{
    build_spawn_memory_admission, spawn_memory_projection_mb_with_overrides,
};
use crate::run_resource_overrides::RunResourceOverrides;
use crate::session_guard::{
    SessionCleanupGuard, write_pre_exec_error_result, write_pre_exec_error_result_with_no_provider,
};

#[derive(Clone, Copy, Default)]
pub(super) struct PipelinePreExecFailureDetails<'a> {
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) task_type: Option<&'a str>,
    pub(super) resource_overrides: RunResourceOverrides,
}

pub(super) fn check_resources_before_spawn(
    config: Option<&ProjectConfig>,
    executor: &Executor,
    project_root: &Path,
    session: &mut MetaSessionState,
    cleanup_guard: &mut Option<SessionCleanupGuard>,
    resource_overrides: RunResourceOverrides,
    task_type: Option<&str>,
) -> anyhow::Result<()> {
    let mut resource_guard = ResourceGuard::new(ResourceLimits {
        min_free_memory_mb: resource_overrides.resolve_min_free_memory_mb(config),
    });
    let projected_spawn_mb =
        spawn_memory_projection_mb_with_overrides(config, executor.tool_name(), resource_overrides);
    if let Err(err) =
        crate::resource_admission::persist_spawn_memory_projection(session, projected_spawn_mb)
    {
        return Err(persist_pipeline_pre_exec_failure(
            project_root,
            session,
            executor.tool_name(),
            err.context("Failed to persist pre-spawn memory projection"),
            cleanup_guard,
            None,
            PipelinePreExecFailureDetails {
                config,
                task_type,
                resource_overrides,
            },
        ));
    }
    let admission =
        build_spawn_memory_admission(project_root, &session.meta_session_id, projected_spawn_mb);

    if let Err(err) =
        resource_guard.check_availability_with_admission(executor.tool_name(), Some(admission))
    {
        return Err(persist_pipeline_pre_exec_failure(
            project_root,
            session,
            executor.tool_name(),
            err,
            cleanup_guard,
            Some("low_memory"),
            PipelinePreExecFailureDetails {
                config,
                task_type,
                resource_overrides,
            },
        ));
    }
    if let Some(cfg) = config {
        resource_guard.check_health(
            resource_overrides.resolve_memory_max_mb(config, executor.tool_name()),
            cfg.sandbox_memory_swap_max_mb(executor.tool_name()),
            60,
        );
    } else if resource_overrides.memory_max_mb.is_some() {
        resource_guard.check_health(
            resource_overrides.resolve_memory_max_mb(None, executor.tool_name()),
            csa_config::default_sandbox_for_tool(executor.tool_name()).memory_swap_max_mb,
            60,
        );
    }
    Ok(())
}

/// Writes the `.fatal-error-markers` sidecar that scopes a session's fatal-error
/// watchdog policy.
///
/// PRECONDITION: the caller MUST already hold the session lock. The write uses
/// `File::create` (truncating), so invoking this before `acquire_lock` let a
/// concurrent invocation that then fails to take the lock still replace a live
/// session's sidecar — silently disabling or broadening the running watchdog
/// policy (#1652 round-7 review finding).
pub(super) fn write_fatal_error_marker_sidecar(
    config: Option<&ProjectConfig>,
    session_dir: &Path,
    project_root: &Path,
    session: &mut MetaSessionState,
    tool_name: &str,
    cleanup_guard: &mut Option<SessionCleanupGuard>,
) -> anyhow::Result<()> {
    csa_process::reset_liveness_scope(session_dir, tool_name).map_err(|err| {
        persist_pipeline_pre_exec_failure(
            project_root,
            session,
            tool_name,
            anyhow::anyhow!(err).context("Failed to reset active liveness scope"),
            cleanup_guard,
            None,
            PipelinePreExecFailureDetails::default(),
        )
    })?;

    let Some(cfg) = config else {
        return Ok(());
    };
    let fatal_error_markers =
        fatal_error_markers_for_tool(&cfg.resources.fatal_error_markers, tool_name);
    csa_process::write_fatal_error_markers(session_dir, &fatal_error_markers).map_err(|err| {
        persist_pipeline_pre_exec_failure(
            project_root,
            session,
            tool_name,
            anyhow::anyhow!(err).context("Failed to write fatal error marker sidecar"),
            cleanup_guard,
            None,
            PipelinePreExecFailureDetails::default(),
        )
    })
}

fn fatal_error_markers_for_tool(config_markers: &[String], tool_name: &str) -> Vec<String> {
    let mut markers = config_markers.to_vec();
    if tool_name == "gemini-cli"
        && !markers.iter().any(|marker| {
            marker.eq_ignore_ascii_case(csa_executor::GEMINI_OAUTH_PROMPT_FATAL_MARKER)
        })
    {
        markers.push(csa_executor::GEMINI_OAUTH_PROMPT_FATAL_MARKER.to_string());
    }
    markers
}

pub(super) fn persist_pipeline_pre_exec_failure(
    project_root: &Path,
    session: &mut MetaSessionState,
    tool_name: &str,
    err: anyhow::Error,
    cleanup_guard: &mut Option<SessionCleanupGuard>,
    termination_reason: Option<&str>,
    details: PipelinePreExecFailureDetails<'_>,
) -> anyhow::Error {
    let no_provider_launch = crate::no_provider_launch::diagnostic_from_error(
        crate::no_provider_launch::NoProviderLaunchContext {
            session,
            tool_name,
            task_type: details.task_type,
            config: details.config,
            resource_overrides: details.resource_overrides,
        },
        &err,
    );
    if let Some(diagnostic) = no_provider_launch {
        write_pre_exec_error_result_with_no_provider(
            project_root,
            &session.meta_session_id,
            tool_name,
            &err,
            diagnostic,
        );
    } else {
        write_pre_exec_error_result(project_root, &session.meta_session_id, tool_name, &err);
    }
    let cleared_admission_projection =
        crate::resource_admission::clear_spawn_memory_projection(session);
    if let Some(reason) = termination_reason {
        session.termination_reason = Some(reason.to_string());
    }
    if termination_reason.is_some() || cleared_admission_projection {
        session.last_accessed = chrono::Utc::now();
        if let Err(save_err) = save_session(session) {
            warn!(
                session = %session.meta_session_id,
                error = %save_err,
                termination_reason = ?termination_reason,
                "Failed to persist pre-exec failure session state"
            );
        }
    }
    if let Some(cg) = cleanup_guard {
        cg.defuse();
    }
    err
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_marker_sidecar_includes_oauth_browser_prompt_marker() {
        let markers = fatal_error_markers_for_tool(&["HTTP 429".to_string()], "gemini-cli");

        assert!(
            markers
                .iter()
                .any(|marker| { marker == csa_executor::GEMINI_OAUTH_PROMPT_FATAL_MARKER })
        );
    }

    #[test]
    fn non_gemini_marker_sidecar_preserves_config_markers_only() {
        let markers = fatal_error_markers_for_tool(&["HTTP 429".to_string()], "codex");

        assert_eq!(markers, vec!["HTTP 429".to_string()]);
    }
}
