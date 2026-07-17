use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_executor::{ExecuteOptions, SandboxContext};
use csa_lock::acquire_lock;
use csa_process::StreamMode;
use csa_resource::isolation_plan::{
    EnforcementMode as ResourceEnforcementMode, IsolationPlanBuilder,
};
use csa_session::MetaSessionState;

use super::clean_room_completion::{
    CleanRoomCompletionPlan, complete_clean_room_error, complete_clean_room_session,
};
use super::session_exec_api::{CleanRoomExecutionLimits, SessionExecutionPolicy};
use super::session_exec_bootstrap::bootstrap_clean_room_session;
use super::session_exec_pre_exec::check_resources_before_spawn;
use crate::pipeline::admitted_executor::DispatchExecutor;
use crate::pipeline::{AdmittedExecutor, SessionExecutionResult};
use crate::run_resource_overrides::RunResourceOverrides;
use crate::session_guard::SessionCleanupGuard;

#[derive(Clone, Copy)]
pub(crate) struct CleanRoomSandboxInput<'a> {
    pub(crate) config: Option<&'a ProjectConfig>,
    pub(crate) tool_name: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) project_root: &'a Path,
    pub(crate) evidence_bundle: &'a Path,
    pub(crate) session_dir: &'a Path,
    pub(crate) idle_timeout_seconds: u64,
    pub(crate) initial_response_timeout_seconds: Option<u64>,
}

pub(super) fn validate_clean_room_sandbox_capability() -> Result<()> {
    let capability = csa_resource::detect_filesystem_capability();
    if capability != csa_resource::FilesystemCapability::Bwrap {
        bail!(
            "clean-room execution requires strict bubblewrap filesystem isolation; detected {capability}"
        );
    }
    Ok(())
}

fn resolve_clean_room_sandbox_options(
    input: CleanRoomSandboxInput<'_>,
    resource_overrides: RunResourceOverrides,
) -> Result<ExecuteOptions> {
    resolve_clean_room_sandbox_options_with_capabilities(
        input,
        resource_overrides,
        csa_resource::detect_filesystem_capability(),
        csa_resource::detect_resource_capability(),
    )
}

pub(crate) fn resolve_clean_room_sandbox_options_with_capabilities(
    input: CleanRoomSandboxInput<'_>,
    resource_overrides: RunResourceOverrides,
    filesystem_capability: csa_resource::FilesystemCapability,
    resource_capability: csa_resource::ResourceCapability,
) -> Result<ExecuteOptions> {
    if filesystem_capability != csa_resource::FilesystemCapability::Bwrap {
        bail!(
            "clean-room execution requires bubblewrap; filesystem capability {filesystem_capability} is not admitted"
        );
    }
    let project_root = canonical_clean_room_path("project root", input.project_root)?;
    let evidence_bundle = canonical_clean_room_path("evidence bundle", input.evidence_bundle)?;
    let session_dir = canonical_clean_room_path("session directory", input.session_dir)?;
    if paths_overlap(&project_root, &evidence_bundle)
        || paths_overlap(&project_root, &session_dir)
        || paths_overlap(&evidence_bundle, &session_dir)
    {
        bail!(
            "clean-room sandbox read/write roots must not overlap: project={}, evidence={}, session={}",
            project_root.display(),
            evidence_bundle.display(),
            session_dir.display()
        );
    }

    let default_resources = csa_config::ResourcesConfig::default();
    let resources = input
        .config
        .map(|config| &config.resources)
        .unwrap_or(&default_resources);
    let memory_max_mb = resource_overrides.resolve_memory_max_mb(input.config, input.tool_name);
    let resource_enforcement = if resource_overrides.has_memory_max_override() {
        ResourceEnforcementMode::Required
    } else {
        match resources.enforcement_mode {
            Some(csa_config::EnforcementMode::Required) => ResourceEnforcementMode::Required,
            Some(csa_config::EnforcementMode::BestEffort) => ResourceEnforcementMode::BestEffort,
            Some(csa_config::EnforcementMode::Off) | None => ResourceEnforcementMode::Off,
        }
    };
    let (memory_swap_max_mb, pids_max) = input.config.map_or((None, None), |config| {
        (
            config.sandbox_memory_swap_max_mb(input.tool_name),
            config.sandbox_pids_max(),
        )
    });
    let (resource_capability, memory_max_mb, memory_swap_max_mb, pids_max) =
        if resource_enforcement == ResourceEnforcementMode::Off {
            (csa_resource::ResourceCapability::None, None, None, None)
        } else {
            (
                resource_capability,
                memory_max_mb,
                memory_swap_max_mb,
                pids_max,
            )
        };
    let mut plan = IsolationPlanBuilder::new(resource_enforcement)
        .with_filesystem_enforcement(ResourceEnforcementMode::Required)
        .with_resource_capability(resource_capability)
        .with_filesystem_capability(filesystem_capability)
        .with_resource_limits(memory_max_mb, memory_swap_max_mb, pids_max)
        .with_writable_path(project_root.clone())
        .with_writable_path(session_dir.clone())
        .with_readable_path(evidence_bundle.clone())
        .with_readonly_project_root(true)
        .with_soft_limit_percent(resources.soft_limit_percent)
        .with_memory_monitor_interval(resources.memory_monitor_interval_seconds)
        .build()
        .context("build strict clean-room isolation plan")?;
    plan.project_root = Some(project_root);
    if !plan.degraded_reasons.is_empty() {
        bail!(
            "clean-room isolation may not degrade: {}",
            plan.degraded_reasons.join("; ")
        );
    }
    if !plan.env_overrides.is_empty() || plan.user_daemon_ipc {
        bail!("clean-room isolation plan contains forbidden ambient capabilities");
    }

    let liveness_dead_seconds = resources
        .liveness_dead_seconds
        .unwrap_or(input.idle_timeout_seconds)
        .max(input.idle_timeout_seconds);
    let options = ExecuteOptions::new(StreamMode::BufferOnly, input.idle_timeout_seconds)
        .with_acp_crash_max_attempts(input.config.map_or_else(
            || csa_config::ExecutionConfig::default().resolved_acp_crash_max_attempts(),
            |config| config.execution.resolved_acp_crash_max_attempts(),
        ))
        .with_liveness_dead_seconds(liveness_dead_seconds)
        .with_stdin_write_timeout_seconds(resources.stdin_write_timeout_seconds)
        .with_acp_init_timeout_seconds(
            input
                .config
                .map(|config| config.acp.init_timeout_seconds)
                .unwrap_or(csa_config::AcpConfig::default().init_timeout_seconds),
        )
        .with_termination_grace_period_seconds(resources.termination_grace_period_seconds)
        .with_initial_response_timeout_seconds(input.initial_response_timeout_seconds)
        .with_sandbox(SandboxContext {
            isolation_plan: plan,
            tool_name: input.tool_name.to_string(),
            session_id: input.session_id.to_string(),
            best_effort: false,
        });
    Ok(options)
}

fn canonical_clean_room_path(label: &str, path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("clean-room {label} must be absolute: {}", path.display());
    }
    path.canonicalize()
        .with_context(|| format!("canonicalize clean-room {label}: {}", path.display()))
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

pub(super) struct CleanRoomRuntimePlan {
    pub(super) effective_prompt: String,
    pub(super) execute_options: ExecuteOptions,
}

struct CleanRoomRuntimeInput<'a> {
    prompt: &'a str,
    project_root: &'a Path,
    evidence_bundle: &'a Path,
    session_dir: &'a Path,
    tool_name: &'a str,
    config: Option<&'a ProjectConfig>,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    resource_overrides: RunResourceOverrides,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub(super) enum RuntimePlan {
    Legacy,
    CleanRoom,
}

#[cfg(test)]
impl RuntimePlan {
    pub(super) const fn effect_names(self) -> &'static [&'static str] {
        match self {
            Self::Legacy => &[
                "context-injection",
                "prompt-guards",
                "hooks",
                "provider-resume",
                "output-spool",
            ],
            Self::CleanRoom => &[
                "exact-prompt",
                "strict-sandbox",
                "runtime-prerequisites",
                "liveness-timeouts",
                "signal-cleanup",
            ],
        }
    }
}

fn prepare_clean_room_runtime(
    input: CleanRoomRuntimeInput<'_>,
    session: &mut MetaSessionState,
) -> Result<CleanRoomRuntimePlan> {
    let mut execute_options = resolve_clean_room_sandbox_options(
        CleanRoomSandboxInput {
            config: input.config,
            tool_name: input.tool_name,
            session_id: &session.meta_session_id,
            project_root: input.project_root,
            evidence_bundle: input.evidence_bundle,
            session_dir: input.session_dir,
            idle_timeout_seconds: input.idle_timeout_seconds,
            initial_response_timeout_seconds: input.initial_response_timeout_seconds,
        },
        input.resource_overrides,
    )?;
    execute_options = execute_options
        .with_error_marker_scan_enabled(false)
        .with_git_push_allowed(false);
    crate::resource_admission_soft_limit::ensure_memory_soft_limit_admission(
        None,
        input.tool_name,
        execute_options
            .sandbox
            .as_ref()
            .map(|sandbox| &sandbox.isolation_plan),
    )?;
    if crate::pipeline_sandbox::record_sandbox_telemetry(
        &execute_options,
        session,
        input
            .resource_overrides
            .resolution_info(input.config, input.tool_name),
    ) {
        csa_session::save_session(session).context("persist clean-room sandbox telemetry")?;
    }
    Ok(CleanRoomRuntimePlan {
        effective_prompt: input.prompt.to_owned(),
        execute_options,
    })
}

pub(super) async fn execute_clean_room_session_core(
    admitted: &AdmittedExecutor,
    tool: &ToolName,
    prompt: &str,
    policy: SessionExecutionPolicy,
    config: Option<&ProjectConfig>,
    global_config: Option<&GlobalConfig>,
    limits: CleanRoomExecutionLimits,
) -> Result<SessionExecutionResult> {
    let SessionExecutionPolicy::CleanRoom(contract) = policy else {
        anyhow::bail!("clean-room core requires CleanRoom session policy");
    };
    let project_root = contract.project_root().to_path_buf();
    let evidence_bundle = contract.evidence_bundle().to_path_buf();
    let executor = admitted.executor();
    let super::session_exec_bootstrap::SessionBootstrap {
        mut session,
        resolved_provider_session_id,
    } = bootstrap_clean_room_session(
        tool,
        &project_root,
        config,
        global_config,
        limits.tier_name(),
    )?;
    debug_assert!(resolved_provider_session_id.is_none());
    let session_dir = csa_session::get_session_dir(&project_root, &session.meta_session_id)?;
    let mut cleanup_guard = Some(SessionCleanupGuard::new(session_dir.clone()));
    let execution_start_time = chrono::Utc::now();

    if let Err(error) = crate::preflight_state_dir::enforce_state_dir_cap(
        global_config,
        Some(&session.meta_session_id),
    ) {
        return fail_clean_room_pre_transport(
            &project_root,
            &mut session,
            executor.tool_name(),
            execution_start_time,
            error,
            &mut cleanup_guard,
        );
    }
    let lock = match acquire_lock(&session_dir, executor.tool_name(), "clean-room execution") {
        Ok(lock) => lock,
        Err(error) => {
            return fail_clean_room_pre_transport(
                &project_root,
                &mut session,
                executor.tool_name(),
                execution_start_time,
                error.context("acquire clean-room session lock"),
                &mut cleanup_guard,
            );
        }
    };
    let _lock = lock;
    if let Some(config) = config
        && let Err(error) = csa_process::write_fatal_error_markers(
            &session_dir,
            &config.resources.fatal_error_markers,
        )
    {
        return fail_clean_room_pre_transport(
            &project_root,
            &mut session,
            executor.tool_name(),
            execution_start_time,
            anyhow::Error::from(error).context("write clean-room fatal-error diagnostics"),
            &mut cleanup_guard,
        );
    }
    check_resources_before_spawn(
        config,
        executor,
        &project_root,
        &mut session,
        &mut cleanup_guard,
        limits.resource_overrides(),
        None,
    )?;
    let default_global;
    let global = match global_config {
        Some(config) => config,
        None => {
            default_global = GlobalConfig::default();
            &default_global
        }
    };
    let _slot = match crate::pipeline::acquire_slot(executor, global) {
        Ok(slot) => slot,
        Err(error) => {
            return fail_clean_room_pre_transport(
                &project_root,
                &mut session,
                executor.tool_name(),
                execution_start_time,
                error.context("reserve clean-room executor slot"),
                &mut cleanup_guard,
            );
        }
    };
    let runtime = match prepare_clean_room_runtime(
        CleanRoomRuntimeInput {
            prompt,
            project_root: &project_root,
            evidence_bundle: &evidence_bundle,
            session_dir: &session_dir,
            tool_name: executor.tool_name(),
            config,
            idle_timeout_seconds: limits.idle_timeout_seconds(),
            initial_response_timeout_seconds: limits.initial_response_timeout_seconds(),
            resource_overrides: limits.resource_overrides(),
        },
        &mut session,
    ) {
        Ok(runtime) => runtime,
        Err(error) => {
            return fail_clean_room_pre_transport(
                &project_root,
                &mut session,
                executor.tool_name(),
                execution_start_time,
                error,
                &mut cleanup_guard,
            );
        }
    };
    admitted.emit_catalog_warning();
    let timeout_diagnostics =
        crate::session_kill_diagnostics::TimeoutDiagnostics::from_execution_options(
            limits
                .wall_timeout()
                .map(|timeout| timeout.as_secs().max(1)),
            limits.idle_timeout_seconds(),
            limits.initial_response_timeout_seconds(),
        );
    let transport = match crate::pipeline_execute::execute_clean_transport_with_signal(
        executor,
        &runtime.effective_prompt,
        &session,
        runtime.execute_options,
        contract.into_command(),
        execution_start_time,
        limits.wall_timeout(),
    )
    .await
    {
        Ok(transport) => transport,
        Err(error) => {
            return fail_clean_room_pre_transport(
                &project_root,
                &mut session,
                executor.tool_name(),
                execution_start_time,
                error,
                &mut cleanup_guard,
            );
        }
    };
    if let Some(guard) = cleanup_guard.as_mut() {
        guard.defuse();
    }
    complete_clean_room_session(
        &project_root,
        &mut session,
        executor.tool_name(),
        transport,
        CleanRoomCompletionPlan {
            execution_start_time,
            timeout_diagnostics,
        },
    )
}

fn fail_clean_room_pre_transport(
    project_root: &std::path::Path,
    session: &mut csa_session::MetaSessionState,
    tool_name: &str,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    error: anyhow::Error,
    cleanup_guard: &mut Option<SessionCleanupGuard>,
) -> Result<SessionExecutionResult> {
    complete_clean_room_error(
        project_root,
        session,
        tool_name,
        execution_start_time,
        &error,
    )?;
    if let Some(guard) = cleanup_guard.as_mut() {
        guard.defuse();
    }
    Err(error)
}
