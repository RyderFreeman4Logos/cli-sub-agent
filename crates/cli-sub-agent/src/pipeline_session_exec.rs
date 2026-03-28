//! Session-bound execution pipeline: resolve-or-create session, run tool, post-process results.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, info, warn};

use super::prompt_guard::emit_prompt_guard_to_caller;
use super::result_contract::{clear_expected_result_toml, enforce_result_toml_path_contract};
use super::{
    MemoryInjectionOptions, ParentSessionSource, SessionExecutionResult,
    resolve_liveness_dead_seconds, resolve_mcp_servers, run_pipeline_hook,
};
use crate::memory_capture;
use crate::pipeline_project_key::resolve_memory_project_key;
use crate::run_helpers::truncate_prompt;
use crate::session_guard::{SessionCleanupGuard, write_pre_exec_error_result};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_executor::Executor;
use csa_hooks::{
    GuardContext, HookEvent, format_guard_output, global_hooks_path, load_hooks_config,
    run_prompt_guards,
};
use csa_lock::acquire_lock;
use csa_process::ExecutionResult;
use csa_resource::{ResourceGuard, ResourceLimits};
use csa_session::{ToolState, create_session, get_session_dir};

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all, fields(tool = %tool, session = ?session_arg))]
pub(crate) async fn execute_with_session(
    executor: &Executor,
    tool: &ToolName,
    prompt: &str,
    session_arg: Option<String>,
    description: Option<String>,
    parent: Option<String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    extra_env: Option<&std::collections::HashMap<String, String>>,
    task_type: Option<&str>,
    tier_name: Option<&str>,
    context_load_options: Option<&csa_executor::ContextLoadOptions>,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    wall_timeout: Option<Duration>,
    memory_injection: Option<&MemoryInjectionOptions>,
    global_config: Option<&GlobalConfig>,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
) -> Result<ExecutionResult> {
    let execution = execute_with_session_and_meta(
        executor,
        tool,
        prompt,
        OutputFormat::Json,
        session_arg,
        description,
        parent,
        project_root,
        config,
        extra_env,
        task_type,
        tier_name,
        context_load_options,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        wall_timeout,
        memory_injection,
        global_config,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
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
    description: Option<String>,
    parent: Option<String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    extra_env: Option<&std::collections::HashMap<String, String>>,
    task_type: Option<&str>,
    tier_name: Option<&str>,
    context_load_options: Option<&csa_executor::ContextLoadOptions>,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    wall_timeout: Option<Duration>,
    memory_injection: Option<&MemoryInjectionOptions>,
    global_config: Option<&GlobalConfig>,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
) -> Result<SessionExecutionResult> {
    execute_with_session_and_meta_with_parent_source(
        executor,
        tool,
        prompt,
        output_format,
        session_arg,
        description,
        parent,
        project_root,
        config,
        extra_env,
        task_type,
        tier_name,
        context_load_options,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        wall_timeout,
        memory_injection,
        global_config,
        ParentSessionSource::ExplicitOrEnv,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all, fields(tool = %tool, parent_session_source = ?parent_session_source))]
pub(crate) async fn execute_with_session_and_meta_with_parent_source(
    executor: &Executor,
    tool: &ToolName,
    prompt: &str,
    output_format: OutputFormat,
    session_arg: Option<String>,
    description: Option<String>,
    parent: Option<String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    extra_env: Option<&std::collections::HashMap<String, String>>,
    task_type: Option<&str>,
    tier_name: Option<&str>,
    context_load_options: Option<&csa_executor::ContextLoadOptions>,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    wall_timeout: Option<Duration>,
    memory_injection: Option<&MemoryInjectionOptions>,
    global_config: Option<&GlobalConfig>,
    parent_session_source: ParentSessionSource,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
) -> Result<SessionExecutionResult> {
    // Check for parent session violation: a child process must not operate on its own session
    if let Some(ref session_id) = session_arg
        && let Ok(env_session) = std::env::var("CSA_SESSION_ID")
        && env_session == *session_id
    {
        return Err(csa_core::error::AppError::ParentSessionViolation.into());
    }
    let memory_project_key = resolve_memory_project_key(project_root);
    let mut resolved_provider_session_id: Option<String> = None;
    let mut session = if let Some(ref session_id) = session_arg {
        let resolution =
            csa_session::resolve_resume_session_id(session_id, parent_session_source)
                .await
                .context("Failed to resolve session")?;
        if let Some(resolved_id) = resolution {
            resolved_provider_session_id = Some(resolved_id.clone());
            session = Some(resolved_id);
        } else {
            session = None;
        }
    } else {
        session = None;
    };

    if session.is_none() {
        session = Some(
            create_session(
                tool,
                project_root,
                config,
                parent,
                description,
                task_type,
                tier_name,
                memory_injection,
                global_config,
            )
            .await
            .context("Failed to create session")?,
        );
    }

    let session_id = session.as_ref().unwrap().clone();
    let session_dir = get_session_dir(&session_id).context("Failed to get session dir")?;
    let tool_state = ToolState::new(tool, session_id.clone());

    let mut resource_limits = ResourceLimits::default();
    if let Some(memory_injection) = memory_injection {
        resource_limits.memory = memory_injection.memory;
    }

    let resource_guard = ResourceGuard::new(resource_limits)
        .await
        .context("Failed to create resource guard")?;

    let lock = acquire_lock(session_id.clone())
        .await
        .context("Failed to acquire lock")?;

    let cleanup_guard = SessionCleanupGuard::new(session_id.clone(), lock);

    let mut execution_result = ExecutionResult::new(tool_state.clone(), output_format);

    let mut extra_writable_paths = Vec::new();
    for path in extra_writable {
        if path.exists() {
            extra_writable_paths.push(path.to_path_buf());
        } else {
            warn!("Extra writable path {} does not exist", path.display());
        }
    }

    let mut env = std::collections::HashMap::new();
    if let Some(extra_env) = extra_env {
        env.extend(extra_env.clone());
    }

    let mut context_load_options = context_load_options.cloned();
    if context_load_options.is_none() {
        context_load_options = Some(csa_executor::ContextLoadOptions::default());
    }

    let execution = executor
        .execute(
            &tool_state,
            &execution_result,
            &mut env,
            &extra_writable_paths,
            context_load_options.as_ref().unwrap(),
            stream_mode,
            idle_timeout_seconds,
            initial_response_timeout_seconds,
            wall_timeout,
            &resource_guard,
            &cleanup_guard,
        )
        .await
        .context("Failed to execute tool")?;

    let execution_result = execution_result.clone();
    let session_execution_result = SessionExecutionResult {
        execution: execution_result,
        session_id: session_id.clone(),
        session_dir: session_dir.clone(),
        tool_state: tool_state.clone(),
        resolved_provider_session_id,
        resource_guard,
        cleanup_guard,
    };

    Ok(session_execution_result)
}