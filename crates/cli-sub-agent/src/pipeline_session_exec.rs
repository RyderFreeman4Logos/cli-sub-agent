use crate::pipeline::{
    DispatchExecutor, MemoryInjectionOptions, ParentSessionSource, SessionCreationMode,
    SessionExecutionResult,
};
use crate::pipeline_project_key::resolve_memory_project_key;
use crate::run_helpers::truncate_prompt;
use crate::run_resource_overrides::RunResourceOverrides;
use crate::session_guard::SessionCleanupGuard;
use crate::startup_env::StartupSubtreeEnv;
use anyhow::{Context, Result};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_lock::acquire_lock;
use csa_session::get_session_dir;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tracing::{info, warn};
#[path = "pipeline_session_exec_clean_room.rs"]
mod clean_room;
#[path = "pipeline_session_exec_clean_room_completion.rs"]
mod clean_room_completion;
#[path = "pipeline_session_exec_runtime_contract.rs"]
mod runtime_contract;
#[path = "pipeline_session_exec_api.rs"]
mod session_exec_api;
#[path = "pipeline_session_exec_audit.rs"]
mod session_exec_audit;
#[path = "pipeline_session_exec_bootstrap.rs"]
mod session_exec_bootstrap;
#[path = "pipeline_session_exec_completion.rs"]
mod session_exec_completion;
#[path = "pipeline_session_exec_memory.rs"]
mod session_exec_memory;
#[path = "pipeline_session_exec_metadata.rs"]
mod session_exec_metadata;
#[path = "pipeline_session_exec_pre_exec.rs"]
mod session_exec_pre_exec;
#[path = "pipeline_session_exec_prompt_guard.rs"]
mod session_exec_prompt_guard;
#[path = "pipeline_session_exec_prompt_inject.rs"]
mod session_exec_prompt_inject;
#[path = "pipeline_session_exec_review_guard.rs"]
mod session_exec_review_guard;
#[path = "pipeline_session_exec_runtime.rs"]
mod session_exec_runtime;
#[path = "pipeline_session_exec_tool_state.rs"]
mod session_exec_tool_state;
#[path = "pipeline_session_exec_write_guard.rs"]
mod session_exec_write_guard;
#[path = "pipeline_session_exec_write_lock.rs"]
mod session_exec_write_lock;
#[path = "pipeline_session_exec_state_preflight.rs"]
mod state_preflight;
use self::session_exec_pre_exec::{
    PipelinePreExecFailureDetails, check_resources_before_spawn, persist_pipeline_pre_exec_failure,
    write_fatal_error_marker_sidecar,
};
use clean_room::execute_clean_room_session_core;
#[cfg(test)]
pub(crate) use clean_room::{
    CleanRoomSandboxInput, resolve_clean_room_sandbox_options_with_capabilities,
};
pub(crate) use session_exec_api::execute_with_session;
#[cfg(test)]
pub(crate) use session_exec_api::execute_with_session_and_meta;
pub(crate) use session_exec_api::{
    CleanRoomExecutionContract, CleanRoomExecutionLimits, execute_clean_room_session,
};

#[cfg(test)]
pub(crate) struct CleanRoomPolicyEffects {
    pub(crate) bootstrap: &'static [&'static str],
    pub(crate) runtime: &'static [&'static str],
    pub(crate) completion: &'static [&'static str],
    pub(crate) forbidden: &'static [&'static str],
}

#[cfg(test)]
pub(crate) fn clean_room_execution_policy_effects() -> CleanRoomPolicyEffects {
    let _legacy_bootstrap_effects = session_exec_bootstrap::BootstrapPlan::Legacy.effect_names();
    let _legacy_runtime_effects = clean_room::RuntimePlan::Legacy.effect_names();
    let _legacy_completion_effects = clean_room_completion::CompletionPlan::Legacy.effect_names();
    CleanRoomPolicyEffects {
        bootstrap: session_exec_bootstrap::BootstrapPlan::CleanRoom.effect_names(),
        runtime: clean_room::RuntimePlan::CleanRoom.effect_names(),
        completion: clean_room_completion::CompletionPlan::CleanRoom.effect_names(),
        forbidden: &[],
    }
}

#[cfg(test)]
pub(crate) fn clean_room_runtime_prompt_for_test(prompt: &str) -> String {
    prompt.to_owned()
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all, fields(tool = %tool, parent_session_source = ?parent_session_source))]
pub(crate) async fn execute_with_session_and_meta_with_parent_source<
    D: DispatchExecutor + ?Sized,
>(
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
    // Trusted CSA-decided subtree model pin (#1741), carried outside generic
    // `extra_env`. The executor applies it after env merges strip pin keys.
    // `None` means CSA did not pin; never source this from request/config env.
    subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
    allow_git_push: bool,
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
    parent_session_source: ParentSessionSource,
    session_creation_mode: SessionCreationMode,
    resource_overrides: RunResourceOverrides,
    no_fs_sandbox: bool,
    allow_user_daemon_ipc: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
    error_marker_scan_override: Option<bool>,
    cli_no_hook_bypass_scan: bool,
    startup_env: &StartupSubtreeEnv,
) -> Result<SessionExecutionResult> {
    let dispatch_executor = executor;
    let executor = dispatch_executor.executor();
    let memory_project_key = resolve_memory_project_key(project_root, startup_env.project_root());
    let session_exec_bootstrap::SessionBootstrap {
        mut session,
        resolved_provider_session_id,
    } = session_exec_bootstrap::bootstrap_session(
        tool,
        prompt,
        session_arg.as_deref(),
        fresh_spawn_preflight_override,
        description,
        parent,
        project_root,
        config,
        global_config,
        task_type,
        tier_name,
        parent_session_source,
        session_creation_mode,
        startup_env,
    )
    .await?;
    let session_dir = get_session_dir(project_root, &session.meta_session_id)?;
    let mut cleanup_guard = if session_arg.is_none() {
        Some(SessionCleanupGuard::new(session_dir.clone()))
    } else {
        None
    };

    // For resumed sessions, clear any stale result.toml before acquiring the
    // worktree lock. This prevents the alive-stale-holder reclaim path from
    // treating a resumed session's old result as evidence of a stuck holder.
    if session_arg.is_some() {
        let result_path = session_dir.join("result.toml");
        let _ = std::fs::remove_file(&result_path);
    }

    let _worktree_write_lock = session_exec_write_lock::acquire_or_persist_failure(
        project_root,
        &mut session,
        executor.tool_name(),
        readonly_project_root,
        &mut cleanup_guard,
    )?;
    let (_log_writer, _log_guard) = match csa_executor::create_session_log_writer(&session_dir) {
        Ok(pair) => pair,
        Err(e) => {
            let err = anyhow::anyhow!(e).context("Failed to create session log writer");
            return Err(persist_pipeline_pre_exec_failure(
                project_root,
                &mut session,
                executor.tool_name(),
                err,
                &mut cleanup_guard,
                None,
                PipelinePreExecFailureDetails::default(),
            ));
        }
    };
    let lock_reason = truncate_prompt(prompt, 80);
    let _lock = match acquire_lock(&session_dir, executor.tool_name(), &lock_reason) {
        Ok(lock) => lock,
        Err(e) => {
            // Pre-exec persistence and `session wait` render the top-level error
            // with Display, so retain the csa-lock owner/path/liveness evidence
            // in that message instead of leaving it only in the source chain.
            let err = anyhow::anyhow!(
                "Failed to acquire lock for session {}: {e:#}",
                session.meta_session_id
            );
            return Err(persist_pipeline_pre_exec_failure(
                project_root,
                &mut session,
                executor.tool_name(),
                err,
                &mut cleanup_guard,
                None,
                PipelinePreExecFailureDetails::default(),
            ));
        }
    };
    // Lock-guarded: see `write_fatal_error_marker_sidecar` precondition (#1652).
    write_fatal_error_marker_sidecar(
        config,
        &session_dir,
        project_root,
        &mut session,
        executor.tool_name(),
        &mut cleanup_guard,
    )?;
    check_resources_before_spawn(
        config,
        executor,
        project_root,
        &mut session,
        &mut cleanup_guard,
        resource_overrides,
        task_type,
    )?;
    if let Some(ref budget) = session.token_budget {
        if budget.is_hard_exceeded() {
            let used = budget.used;
            let allocated = budget.allocated;
            let pct = budget.usage_pct();
            let err = anyhow::anyhow!(
                "token budget exhausted before execution: used={used} allocated={allocated} pct={pct}"
            );
            return Err(persist_pipeline_pre_exec_failure(
                project_root,
                &mut session,
                executor.tool_name(),
                err,
                &mut cleanup_guard,
                None,
                PipelinePreExecFailureDetails::default(),
            ));
        }
        if budget.is_turns_exceeded(session.turn_count) {
            warn!(
                session = %session.meta_session_id,
                turn_count = session.turn_count,
                max_turns = budget.max_turns.unwrap_or(0),
                "Max turns already exceeded — advisory only, execution continues"
            );
        }
        if budget.is_soft_exceeded() {
            warn!(
                session = %session.meta_session_id,
                used = budget.used,
                allocated = budget.allocated,
                pct = budget.usage_pct(),
                "Token budget soft threshold exceeded — approaching limit"
            );
        }
    }
    info!("Executing in session: {}", session.meta_session_id);
    let runtime = session_exec_runtime::prepare_session_runtime(
        session_exec_runtime::SessionRuntimeInput {
            executor,
            tool,
            prompt,
            session_arg: session_arg.as_deref(),
            fresh_spawn_preflight_override,
            project_root,
            session_dir: &session_dir,
            config,
            extra_env,
            subtree_pin,
            allow_git_push,
            task_type,
            context_load_options,
            stream_mode,
            idle_timeout_seconds,
            initial_response_timeout_seconds,
            wall_timeout,
            memory_injection,
            global_config,
            pre_session_hook,
            resource_overrides,
            no_fs_sandbox,
            allow_user_daemon_ipc,
            readonly_project_root,
            extra_writable,
            extra_readable,
            error_marker_scan_override,
            cli_no_hook_bypass_scan,
            startup_env,
            resolved_provider_session_id: &resolved_provider_session_id,
            memory_project_key: memory_project_key.as_deref(),
        },
        &mut session,
        &mut cleanup_guard,
    )
    .await?;
    let session_exec_runtime::SessionRuntimePlan {
        effective_prompt,
        tool_state,
        execute_options,
        session_config,
        completion,
    } = runtime;
    let execution_start_time = completion.execution_start_time;
    dispatch_executor.emit_catalog_warning();
    let transport_result = crate::pipeline_execute::execute_transport_with_signal(
        executor,
        &effective_prompt,
        tool_state.as_ref(),
        &session,
        completion.merged_env_ref(),
        execute_options,
        session_config,
        project_root,
        &mut cleanup_guard,
        execution_start_time,
        wall_timeout,
    )
    .await
    .with_context(|| format!("meta_session_id={}", session.meta_session_id))?;
    if let Some(ref mut guard) = cleanup_guard {
        guard.defuse();
    }
    session_exec_completion::complete_session_execution(
        session_exec_completion::CompletionInput {
            executor,
            tool,
            prompt,
            output_format: &output_format,
            task_type,
            readonly_project_root,
            project_root,
            config,
            global_config,
            session_dir: &session_dir,
            memory_project_key,
            effective_prompt,
            plan: completion,
            transport_result,
        },
        &mut session,
    )
    .await
}
