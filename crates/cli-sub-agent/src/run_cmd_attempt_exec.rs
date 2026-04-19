//! Execution helpers for the `csa run` loop.
//!
//! Extracted from `run_cmd_attempt.rs` to keep module sizes manageable.

use std::path::{Path, PathBuf};

use anyhow::Result;
use tempfile::TempDir;
use tracing::info;

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_executor::Executor;

use crate::pipeline;
use crate::run_cmd_fork::{ForkResolution, cleanup_pre_created_fork_session};

pub(super) enum AttemptExecution {
    Finished(Result<csa_process::ExecutionResult, anyhow::Error>),
    Exit(i32),
    TimedOut,
}

pub(super) struct EphemeralRunRequest<'a> {
    pub(super) executor: &'a Executor,
    pub(super) effective_prompt: &'a str,
    pub(super) project_root: &'a Path,
    pub(super) extra_env: Option<&'a std::collections::HashMap<String, String>>,
    pub(super) stream_mode: csa_process::StreamMode,
    pub(super) idle_timeout_seconds: u64,
    pub(super) initial_response_timeout_seconds: Option<u64>,
}

pub(super) async fn run_ephemeral_with_timeout(
    request: EphemeralRunRequest<'_>,
    timeout_duration: std::time::Duration,
) -> Result<AttemptExecution> {
    let _temp_dir = TempDir::new()?;
    info!(
        "Ephemeral session (metadata: {:?}, cwd: {})",
        _temp_dir.path(),
        request.project_root.display()
    );
    let execution = match tokio::time::timeout(
        timeout_duration,
        request.executor.execute_in(
            request.effective_prompt,
            request.project_root,
            request.extra_env,
            request.stream_mode,
            request.idle_timeout_seconds,
            request.initial_response_timeout_seconds,
        ),
    )
    .await
    {
        Ok(result) => AttemptExecution::Finished(result),
        Err(_) => AttemptExecution::TimedOut,
    };
    Ok(execution)
}

pub(super) async fn run_ephemeral_without_timeout(
    request: EphemeralRunRequest<'_>,
) -> Result<AttemptExecution> {
    let _temp_dir = TempDir::new()?;
    info!(
        "Ephemeral session (metadata: {:?}, cwd: {})",
        _temp_dir.path(),
        request.project_root.display()
    );
    Ok(AttemptExecution::Finished(
        request
            .executor
            .execute_in(
                request.effective_prompt,
                request.project_root,
                request.extra_env,
                request.stream_mode,
                request.idle_timeout_seconds,
                request.initial_response_timeout_seconds,
            )
            .await,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_persistent_with_timeout(
    executor: &Executor,
    current_tool: &ToolName,
    effective_prompt: &str,
    output_format: OutputFormat,
    effective_session_arg: Option<String>,
    description: Option<String>,
    skill_session_tag: Option<String>,
    parent: Option<String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    extra_env: Option<&std::collections::HashMap<String, String>>,
    resolved_tier_name: Option<&str>,
    context_load_options: Option<&csa_executor::ContextLoadOptions>,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    timeout_duration: std::time::Duration,
    memory_injection: &pipeline::MemoryInjectionOptions,
    global_config: &GlobalConfig,
    fork_resolution: Option<&ForkResolution>,
    executed_session_id: &mut Option<String>,
    pre_created_fork_session_id: &mut Option<String>,
    no_fs_sandbox: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
) -> Result<AttemptExecution> {
    match tokio::time::timeout(
        timeout_duration,
        execute_persistent(
            executor,
            current_tool,
            effective_prompt,
            output_format,
            effective_session_arg,
            description,
            skill_session_tag,
            parent,
            project_root,
            config,
            extra_env,
            resolved_tier_name,
            context_load_options,
            stream_mode,
            idle_timeout_seconds,
            initial_response_timeout_seconds,
            Some(timeout_duration),
            memory_injection,
            global_config,
            fork_resolution,
            executed_session_id,
            pre_created_fork_session_id,
            no_fs_sandbox,
            extra_writable,
            extra_readable,
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Ok(AttemptExecution::TimedOut),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_persistent_without_timeout(
    executor: &Executor,
    current_tool: &ToolName,
    effective_prompt: &str,
    output_format: OutputFormat,
    effective_session_arg: Option<String>,
    description: Option<String>,
    skill_session_tag: Option<String>,
    parent: Option<String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    extra_env: Option<&std::collections::HashMap<String, String>>,
    resolved_tier_name: Option<&str>,
    context_load_options: Option<&csa_executor::ContextLoadOptions>,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    memory_injection: &pipeline::MemoryInjectionOptions,
    global_config: &GlobalConfig,
    fork_resolution: Option<&ForkResolution>,
    executed_session_id: &mut Option<String>,
    pre_created_fork_session_id: &mut Option<String>,
    no_fs_sandbox: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
) -> Result<AttemptExecution> {
    execute_persistent(
        executor,
        current_tool,
        effective_prompt,
        output_format,
        effective_session_arg,
        description,
        skill_session_tag,
        parent,
        project_root,
        config,
        extra_env,
        resolved_tier_name,
        context_load_options,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        None,
        memory_injection,
        global_config,
        fork_resolution,
        executed_session_id,
        pre_created_fork_session_id,
        no_fs_sandbox,
        extra_writable,
        extra_readable,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_persistent(
    executor: &Executor,
    current_tool: &ToolName,
    effective_prompt: &str,
    output_format: OutputFormat,
    effective_session_arg: Option<String>,
    description: Option<String>,
    skill_session_tag: Option<String>,
    parent: Option<String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    extra_env: Option<&std::collections::HashMap<String, String>>,
    resolved_tier_name: Option<&str>,
    context_load_options: Option<&csa_executor::ContextLoadOptions>,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    remaining_run_timeout: Option<std::time::Duration>,
    memory_injection: &pipeline::MemoryInjectionOptions,
    global_config: &GlobalConfig,
    fork_resolution: Option<&ForkResolution>,
    executed_session_id: &mut Option<String>,
    pre_created_fork_session_id: &mut Option<String>,
    no_fs_sandbox: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
) -> Result<AttemptExecution> {
    let effective_description = if let Some(fork_res) = fork_resolution {
        description.clone().or_else(|| {
            Some(format!(
                "fork of {}",
                fork_res
                    .source_session_id
                    .get(..8)
                    .unwrap_or(&fork_res.source_session_id)
            ))
        })
    } else {
        description.or(skill_session_tag)
    };
    let effective_parent = if let Some(fork_res) = fork_resolution {
        Some(fork_res.source_session_id.clone())
    } else {
        parent
    };

    let execution = match pipeline::execute_with_session_and_meta(
        executor,
        current_tool,
        effective_prompt,
        output_format,
        effective_session_arg.clone(),
        effective_description,
        effective_parent,
        project_root,
        config,
        extra_env,
        Some("run"),
        resolved_tier_name,
        context_load_options,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        remaining_run_timeout,
        Some(memory_injection),
        Some(global_config),
        no_fs_sandbox,
        false, // readonly_project_root: `csa run` allows writes
        extra_writable,
        extra_readable,
    )
    .await
    {
        Ok(session_result) => {
            *executed_session_id = Some(session_result.meta_session_id);
            AttemptExecution::Finished(Ok(session_result.execution))
        }
        Err(e) => {
            let error_msg = e.to_string();
            if error_msg.contains("Session locked by PID")
                && matches!(output_format, OutputFormat::Json)
            {
                cleanup_pre_created_fork_session(pre_created_fork_session_id, project_root);
                let json_error = serde_json::json!({
                    "error": "session_locked",
                    "session_id": effective_session_arg.unwrap_or_else(|| "(new)".to_string()),
                    "tool": current_tool.as_str(),
                    "message": error_msg
                });
                println!("{}", serde_json::to_string_pretty(&json_error)?);
                AttemptExecution::Exit(1)
            } else {
                AttemptExecution::Finished(Err(e))
            }
        }
    };

    Ok(execution)
}
