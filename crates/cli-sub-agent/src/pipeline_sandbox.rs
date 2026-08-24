//! Sandbox resolution and telemetry for the execution pipeline.
//!
//! Handles enforcement mode checking, capability detection, config resolution,
//! and first-turn telemetry recording.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use csa_config::ProjectConfig;
use csa_executor::{ExecuteOptions, SandboxContext};
use csa_process::StreamMode;
use csa_resource::isolation_plan::{
    EnforcementMode as ResourceEnforcementMode, IsolationPlan, IsolationPlanBuilder,
};
use csa_session::MetaSessionState;
use serde::Serialize;
use tracing::{info, warn};

use crate::run_resource_overrides::RunResourceOverrides;

#[cfg(test)]
use crate::pipeline::{
    CleanRoomSandboxInput, resolve_clean_room_sandbox_options_with_capabilities,
};

#[path = "pipeline_sandbox_memory_balloon.rs"]
mod memory_balloon;
#[path = "pipeline_sandbox_memory_override.rs"]
mod memory_override;
#[path = "pipeline_sandbox_session_dir.rs"]
mod session_dir;
#[path = "pipeline_sandbox_spawn_admission.rs"]
mod spawn_admission;
#[path = "pipeline_sandbox_writable.rs"]
mod writable_sources;
use session_dir::resolve_session_dir_for_sandbox;
use writable_sources::add_execution_env_writable_paths;

#[path = "pipeline_sandbox_resolution.rs"]
mod resolution;
use resolution::resolve_sandbox_options_with_capability_source;

pub(crate) use memory_balloon::maybe_inflate_balloon;
#[cfg(test)]
pub(crate) use memory_balloon::should_skip_balloon_prewarm;
pub(crate) use spawn_admission::resource_capability_for_spawn_admission;

/// Outcome of sandbox resolution — either enriched options or a fatal error string
/// (for `Required` mode with no capability).
pub(crate) enum SandboxResolution {
    /// Options ready (may or may not contain sandbox context).
    Ok(Box<ExecuteOptions>),
    /// Sandbox is required but no capability was detected; caller must bail.
    RequiredButUnavailable(String),
}

/// Sandbox resolution inputs for one session spawn.
pub(crate) struct SandboxResolveInput<'a> {
    pub(crate) config: Option<&'a ProjectConfig>,
    pub(crate) tool_name: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) project_root: &'a Path,
    pub(crate) stream_mode: StreamMode,
    pub(crate) idle_timeout_seconds: u64,
    pub(crate) liveness_dead_seconds: u64,
    pub(crate) initial_response_timeout_seconds: Option<u64>,
    pub(crate) no_fs_sandbox: bool,
    pub(crate) allow_user_daemon_ipc: bool,
    pub(crate) readonly_project_root: bool,
    pub(crate) extra_writable: &'a [PathBuf],
    pub(crate) extra_readable: &'a [PathBuf],
    pub(crate) execution_env: Option<&'a HashMap<String, String>>,
}

pub(crate) fn validate_run_extra_writable_sources_exist(
    config: Option<&ProjectConfig>,
    project_root: &Path,
    no_fs_sandbox: bool,
    extra_writable: &[PathBuf],
) -> Result<(), String> {
    if no_fs_sandbox {
        return Ok(());
    }
    if !extra_writable.is_empty() {
        writable_sources::resolve_and_prepare_writable_sources(
            extra_writable,
            project_root,
            "--extra-writable",
        )?;
    }
    if let Some(cfg) = config
        && !cfg.filesystem_sandbox.extra_writable.is_empty()
    {
        writable_sources::resolve_config_extra_writable_sources(cfg, project_root)?;
    }
    Ok(())
}

/// Resolve sandbox configuration from project config and enforcement mode.
///
/// Returns `SandboxResolution::Ok` with the options (possibly enriched with
/// `SandboxContext`) or `SandboxResolution::RequiredButUnavailable` when
/// enforcement is `Required` but the host lacks both cgroup v2 and setrlimit.
///
/// When `no_fs_sandbox` is `true`, filesystem isolation is forcibly disabled
/// regardless of config (equivalent to `enforcement_mode = "off"` for FS only).
///
/// When `readonly_project_root` is `true`, the project root is mounted read-only
/// via bwrap `--ro-bind` instead of `--bind`. Used by review/debate to prevent
/// the tool from modifying project files.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_sandbox_options(
    config: Option<&ProjectConfig>,
    tool_name: &str,
    session_id: &str,
    project_root: &Path,
    stream_mode: StreamMode,
    idle_timeout_seconds: u64,
    liveness_dead_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
) -> SandboxResolution {
    resolve_sandbox_options_with_overrides(
        SandboxResolveInput {
            config,
            tool_name,
            session_id,
            project_root,
            stream_mode,
            idle_timeout_seconds,
            liveness_dead_seconds,
            initial_response_timeout_seconds,
            no_fs_sandbox,
            allow_user_daemon_ipc: false,
            readonly_project_root,
            extra_writable,
            extra_readable,
            execution_env: None,
        },
        RunResourceOverrides::absent(),
    )
}

pub(crate) fn resolve_sandbox_options_with_overrides(
    input: SandboxResolveInput<'_>,
    resource_overrides: RunResourceOverrides,
) -> SandboxResolution {
    let session_dir = resolve_session_dir_for_sandbox(input.project_root, input.session_id);
    resolve_sandbox_options_with_capability_source(
        input,
        resource_overrides,
        csa_resource::detect_resource_capability,
        csa_resource::detect_filesystem_capability,
        Some(session_dir),
    )
}

pub(crate) fn resolve_pre_session_sandbox_options_with_overrides(
    input: SandboxResolveInput<'_>,
    resource_overrides: RunResourceOverrides,
) -> SandboxResolution {
    resolve_sandbox_options_with_capability_source(
        input,
        resource_overrides,
        csa_resource::detect_resource_capability,
        csa_resource::detect_filesystem_capability,
        None,
    )
}

#[cfg(test)]
pub(crate) fn resolve_sandbox_options_with_capabilities(
    input: SandboxResolveInput<'_>,
    resource_overrides: RunResourceOverrides,
    resource_capability: csa_resource::ResourceCapability,
    filesystem_capability: csa_resource::FilesystemCapability,
) -> SandboxResolution {
    let session_dir = resolve_session_dir_for_sandbox(input.project_root, input.session_id);
    resolve_sandbox_options_with_capability_source(
        input,
        resource_overrides,
        || resource_capability,
        || filesystem_capability,
        Some(session_dir),
    )
}

include!("pipeline_sandbox_telemetry.rs");
#[cfg(test)]
#[path = "pipeline_sandbox_writable_tests.rs"]
mod writable_tests;

#[cfg(test)]
#[path = "pipeline_sandbox_cargo_target_tests.rs"]
mod cargo_target_tests;

#[cfg(test)]
#[path = "pipeline_sandbox_memory_override_tests.rs"]
mod memory_override_tests;

#[cfg(test)]
#[path = "pipeline_sandbox_tests.rs"]
mod tests;
