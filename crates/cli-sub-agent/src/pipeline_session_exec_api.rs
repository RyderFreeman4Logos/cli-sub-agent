use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_executor::Executor;
use csa_process::ExecutionResult;

use super::execute_with_session_and_meta_with_parent_source;
use crate::pipeline::{
    MemoryInjectionOptions, ParentSessionSource, SessionCreationMode, SessionExecutionResult,
};
use crate::run_resource_overrides::RunResourceOverrides;
use crate::startup_env::StartupSubtreeEnv;

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all, fields(tool = %tool, session = ?session_arg))]
pub(crate) async fn execute_with_session(
    executor: &Executor,
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
pub(crate) async fn execute_with_session_and_meta(
    executor: &Executor,
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
