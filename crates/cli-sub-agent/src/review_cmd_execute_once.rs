use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_executor::Executor;

use crate::run_resource_overrides::RunResourceOverrides;
use crate::startup_env::StartupSubtreeEnv;

use super::failures::enforce_review_artifact_contract;

const REVIEWER_SUB_SESSION_TASK_TYPE: &str = "reviewer_sub_session";

#[allow(clippy::too_many_arguments)]
async fn execute_review_once(
    executor: &Executor,
    tool: &ToolName,
    effective_prompt: &str,
    session: Option<String>,
    description: String,
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
    extra_env: Option<&HashMap<String, String>>,
    subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
    tier_name: Option<&str>,
    global_config: &GlobalConfig,
    pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
    error_marker_scan_override: Option<bool>,
    resource_overrides: RunResourceOverrides,
    startup_env: &StartupSubtreeEnv,
) -> Result<crate::pipeline::SessionExecutionResult> {
    crate::pipeline::execute_with_session_and_meta_with_parent_source(
        executor,
        tool,
        effective_prompt,
        OutputFormat::Json,
        session,
        false,
        Some(description),
        None,
        project_root,
        project_config,
        extra_env,
        subtree_pin,
        false,
        Some(REVIEWER_SUB_SESSION_TASK_TYPE),
        tier_name,
        None,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        None,
        None,
        Some(global_config),
        pre_session_hook,
        crate::pipeline::ParentSessionSource::ExplicitOnly,
        crate::pipeline::SessionCreationMode::DaemonManaged,
        resource_overrides,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
        extra_readable,
        error_marker_scan_override,
        false,
        startup_env,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_review_once_with_artifact_guard(
    executor: &Executor,
    tool: &ToolName,
    effective_prompt: &str,
    session: Option<String>,
    description: String,
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
    extra_env: Option<&HashMap<String, String>>,
    subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
    tier_name: Option<&str>,
    global_config: &GlobalConfig,
    pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
    error_marker_scan_override: Option<bool>,
    resource_overrides: RunResourceOverrides,
    startup_env: &StartupSubtreeEnv,
) -> Result<crate::pipeline::SessionExecutionResult> {
    let invocation_started_at = Utc::now();
    match execute_review_once(
        executor,
        tool,
        effective_prompt,
        session,
        description,
        project_root,
        project_config,
        extra_env,
        subtree_pin,
        tier_name,
        global_config,
        pre_session_hook,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
        extra_readable,
        error_marker_scan_override,
        resource_overrides,
        startup_env,
    )
    .await
    {
        Ok(mut execution) => {
            enforce_review_artifact_contract(
                project_root,
                tool,
                invocation_started_at,
                Some(&mut execution),
                None,
            )?;
            Ok(execution)
        }
        Err(err) => {
            enforce_review_artifact_contract(
                project_root,
                tool,
                invocation_started_at,
                None,
                Some(&err),
            )?;
            Err(err)
        }
    }
}
