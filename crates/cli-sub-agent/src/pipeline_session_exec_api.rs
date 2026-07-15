use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_executor::command_isolation::CleanCommandContract;
use csa_process::ExecutionResult;

use super::{DispatchExecutor, execute_with_session_and_meta_with_parent_source};
use crate::pipeline::{
    MemoryInjectionOptions, ParentSessionSource, SessionCreationMode, SessionExecutionResult,
};
use crate::run_resource_overrides::RunResourceOverrides;
use crate::startup_env::StartupSubtreeEnv;

#[derive(Debug, Clone)]
pub(crate) struct CleanRoomExecutionContract {
    project_root: PathBuf,
    evidence_bundle: PathBuf,
    command: CleanCommandContract,
}

impl CleanRoomExecutionContract {
    #[allow(dead_code)]
    pub(crate) fn try_new(
        project_root: impl Into<PathBuf>,
        evidence_bundle: impl Into<PathBuf>,
        command: CleanCommandContract,
    ) -> Result<Self> {
        let project_root = canonical_existing_path("clean-room project root", project_root.into())?;
        if !project_root.is_dir() {
            bail!(
                "clean-room project root must be an existing directory: {}",
                project_root.display()
            );
        }
        let evidence_bundle =
            canonical_existing_path("clean-room evidence bundle", evidence_bundle.into())?;
        if paths_overlap(&project_root, &evidence_bundle) {
            bail!(
                "clean-room evidence bundle must not overlap the project root: project={}, evidence={}",
                project_root.display(),
                evidence_bundle.display()
            );
        }
        let command_cwd = command
            .working_directory()
            .as_path()
            .canonicalize()
            .context("canonicalize typed clean command working directory")?;
        if command_cwd != project_root {
            bail!(
                "typed clean command working directory must equal the validated clean-room project root: command={}, project={}",
                command_cwd.display(),
                project_root.display()
            );
        }
        let program = command.program().as_path();
        if !program.is_file() {
            bail!(
                "typed clean command program must be an existing file: {}",
                program.display()
            );
        }
        Ok(Self {
            project_root,
            evidence_bundle,
            command,
        })
    }

    pub(crate) fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub(crate) fn evidence_bundle(&self) -> &Path {
        &self.evidence_bundle
    }

    pub(super) fn into_command(self) -> CleanCommandContract {
        self.command
    }
}

fn canonical_existing_path(label: &str, path: PathBuf) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("{label} must be absolute: {}", path.display());
    }
    let metadata = std::fs::metadata(&path)
        .with_context(|| format!("{label} must exist: {}", path.display()))?;
    if !metadata.is_file() && !metadata.is_dir() {
        bail!("{label} must be a file or directory: {}", path.display());
    }
    path.canonicalize()
        .with_context(|| format!("canonicalize {label}: {}", path.display()))
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

#[derive(Debug, Clone)]
pub(crate) struct CleanRoomExecutionLimits {
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    wall_timeout: Option<Duration>,
    resource_overrides: RunResourceOverrides,
    tier_name: Option<String>,
}

impl CleanRoomExecutionLimits {
    #[allow(dead_code)]
    pub(crate) fn try_new(
        idle_timeout_seconds: u64,
        initial_response_timeout_seconds: Option<u64>,
        wall_timeout: Option<Duration>,
        resource_overrides: RunResourceOverrides,
        tier_name: Option<String>,
    ) -> Result<Self> {
        if idle_timeout_seconds == 0 {
            bail!("clean-room idle timeout must be greater than zero");
        }
        if initial_response_timeout_seconds == Some(0) {
            bail!("clean-room initial-response timeout must be greater than zero");
        }
        if wall_timeout == Some(Duration::ZERO) {
            bail!("clean-room wall timeout must be greater than zero");
        }
        Ok(Self {
            idle_timeout_seconds,
            initial_response_timeout_seconds,
            wall_timeout,
            resource_overrides,
            tier_name,
        })
    }

    pub(super) fn idle_timeout_seconds(&self) -> u64 {
        self.idle_timeout_seconds
    }

    pub(super) fn initial_response_timeout_seconds(&self) -> Option<u64> {
        self.initial_response_timeout_seconds
    }

    pub(super) fn wall_timeout(&self) -> Option<Duration> {
        self.wall_timeout
    }

    pub(super) fn resource_overrides(&self) -> RunResourceOverrides {
        self.resource_overrides
    }

    pub(super) fn tier_name(&self) -> Option<&str> {
        self.tier_name.as_deref()
    }
}

pub(super) enum SessionExecutionPolicy {
    #[allow(dead_code)]
    Legacy,
    CleanRoom(CleanRoomExecutionContract),
}

#[allow(dead_code)]
pub(crate) async fn execute_clean_room_session(
    admitted: &crate::pipeline::AdmittedExecutor,
    tool: &ToolName,
    prompt: &str,
    contract: CleanRoomExecutionContract,
    config: Option<&ProjectConfig>,
    global_config: Option<&GlobalConfig>,
    limits: CleanRoomExecutionLimits,
) -> Result<SessionExecutionResult> {
    let admitted_identity = admitted.resolved_model_spec();
    if admitted_identity.tool != tool.as_str() || admitted.tool_name() != tool.as_str() {
        bail!(
            "clean-room tool must match the catalog-admitted executor identity: requested={}, admitted={}",
            tool,
            admitted_identity.tool
        );
    }
    super::clean_room::validate_clean_room_sandbox_capability()?;
    super::execute_clean_room_session_core(
        admitted,
        tool,
        prompt,
        SessionExecutionPolicy::CleanRoom(contract),
        config,
        global_config,
        limits,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all, fields(tool = %tool, session = ?session_arg))]
pub(crate) async fn execute_with_session<D: DispatchExecutor + ?Sized>(
    executor: &D,
    tool: &ToolName,
    prompt: &str,
    session_arg: Option<String>,
    fresh_spawn_preflight_override: bool,
    description: Option<String>,
    parent: Option<String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    extra_env: Option<&std::collections::HashMap<String, String>>,
    subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
    task_type: Option<&str>,
    tier_name: Option<&str>,
    context_load_options: Option<&csa_executor::ContextLoadOptions>,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    wall_timeout: Option<Duration>,
    memory_injection: Option<&MemoryInjectionOptions>,
    global_config: Option<&GlobalConfig>,
    pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
    error_marker_scan_override: Option<bool>,
    cli_no_hook_bypass_scan: bool,
    startup_env: &StartupSubtreeEnv,
) -> Result<ExecutionResult> {
    let execution = execute_with_session_and_meta(
        executor,
        tool,
        prompt,
        OutputFormat::Json,
        session_arg,
        fresh_spawn_preflight_override,
        description,
        parent,
        project_root,
        config,
        extra_env,
        subtree_pin,
        task_type,
        tier_name,
        context_load_options,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        wall_timeout,
        memory_injection,
        global_config,
        pre_session_hook,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
        extra_readable,
        error_marker_scan_override,
        cli_no_hook_bypass_scan,
        startup_env,
    )
    .await?;
    Ok(execution.execution)
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all, fields(tool = %tool))]
pub(crate) async fn execute_with_session_and_meta<D: DispatchExecutor + ?Sized>(
    executor: &D,
    tool: &ToolName,
    prompt: &str,
    output_format: OutputFormat,
    session_arg: Option<String>,
    fresh_spawn_preflight_override: bool,
    description: Option<String>,
    parent: Option<String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    extra_env: Option<&std::collections::HashMap<String, String>>,
    subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
    task_type: Option<&str>,
    tier_name: Option<&str>,
    context_load_options: Option<&csa_executor::ContextLoadOptions>,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    wall_timeout: Option<Duration>,
    memory_injection: Option<&MemoryInjectionOptions>,
    global_config: Option<&GlobalConfig>,
    pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
    error_marker_scan_override: Option<bool>,
    cli_no_hook_bypass_scan: bool,
    startup_env: &StartupSubtreeEnv,
) -> Result<SessionExecutionResult> {
    execute_with_session_and_meta_with_parent_source(
        executor,
        tool,
        prompt,
        output_format,
        session_arg,
        fresh_spawn_preflight_override,
        description,
        parent,
        project_root,
        config,
        extra_env,
        subtree_pin,
        false,
        task_type,
        tier_name,
        context_load_options,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        wall_timeout,
        memory_injection,
        global_config,
        pre_session_hook,
        ParentSessionSource::ExplicitOrEnv,
        SessionCreationMode::DaemonManaged,
        RunResourceOverrides::default(),
        no_fs_sandbox,
        false,
        readonly_project_root,
        extra_writable,
        extra_readable,
        error_marker_scan_override,
        cli_no_hook_bypass_scan,
        startup_env,
    )
    .await
}
