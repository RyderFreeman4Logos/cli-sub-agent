use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_executor::{ExecuteOptions, Executor, SessionConfig};
use csa_hooks::{HookEvent, HooksConfig, global_hooks_path, load_hooks_config};
use csa_session::{MetaSessionState, ToolState};
use tracing::{debug, info, warn};

use super::super::prompt_cache::PromptAssembly;
use super::super::result_contract::clear_expected_result_artifacts_for_prompt;
use super::super::session_exec_failover::apply_transport_failover_overrides;
use super::session_exec_pre_exec::persist_pipeline_pre_exec_failure;
use super::session_exec_tool_state::ensure_tool_state_initialized;
use super::{
    session_exec_audit, session_exec_memory, session_exec_metadata, session_exec_prompt_guard,
    session_exec_prompt_inject, state_preflight,
};
use crate::edit_restriction_guard::{NewFileGuard, TrackedFileEditGuard};
use crate::pipeline::{
    MemoryInjectionOptions, resolve_liveness_dead_seconds, resolve_mcp_servers, run_pipeline_hook,
};
use crate::session_guard::SessionCleanupGuard;
use crate::startup_env::StartupSubtreeEnv;

pub(super) struct SessionRuntimeInput<'a> {
    pub(super) executor: &'a Executor,
    pub(super) tool: &'a ToolName,
    pub(super) prompt: &'a str,
    pub(super) session_arg: Option<&'a str>,
    pub(super) fresh_spawn_preflight_override: bool,
    pub(super) project_root: &'a Path,
    pub(super) session_dir: &'a Path,
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) extra_env: Option<&'a HashMap<String, String>>,
    pub(super) subtree_pin: Option<&'a csa_core::env::SubtreeModelPin>,
    pub(super) allow_git_push: bool,
    pub(super) task_type: Option<&'a str>,
    pub(super) context_load_options: Option<&'a csa_executor::ContextLoadOptions>,
    pub(super) stream_mode: csa_process::StreamMode,
    pub(super) idle_timeout_seconds: u64,
    pub(super) initial_response_timeout_seconds: Option<u64>,
    pub(super) memory_injection: Option<&'a MemoryInjectionOptions>,
    pub(super) global_config: Option<&'a GlobalConfig>,
    pub(super) pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    pub(super) no_fs_sandbox: bool,
    pub(super) readonly_project_root: bool,
    pub(super) extra_writable: &'a [PathBuf],
    pub(super) extra_readable: &'a [PathBuf],
    pub(super) error_marker_scan_override: Option<bool>,
    pub(super) cli_no_hook_bypass_scan: bool,
    pub(super) startup_env: &'a StartupSubtreeEnv,
    pub(super) resolved_provider_session_id: &'a Option<String>,
    pub(super) memory_project_key: Option<&'a str>,
}

pub(super) struct SessionRuntimePlan {
    pub(super) effective_prompt: String,
    pub(super) tool_state: Option<ToolState>,
    pub(super) execute_options: ExecuteOptions,
    pub(super) session_config: Option<SessionConfig>,
    pub(super) completion: SessionCompletionPlan,
}

pub(super) struct SessionCompletionPlan {
    pub(super) merged_env: HashMap<String, String>,
    pub(super) hooks_config: HooksConfig,
    pub(super) sessions_root: String,
    pub(super) edit_guard: Option<TrackedFileEditGuard>,
    pub(super) new_file_guard: Option<NewFileGuard>,
    pub(super) result_file_cleared: bool,
    pub(super) execution_start_time: chrono::DateTime<chrono::Utc>,
    pub(super) commit_guard_enabled: bool,
    pub(super) require_commit_on_mutation: bool,
    pub(super) hook_bypass_scan_enabled: bool,
    pub(super) is_git: bool,
    pub(super) inside_git_worktree: bool,
    pub(super) pre_run_workspace: Option<crate::run_cmd::GitWorkspaceSnapshot>,
    pub(super) pre_exec_snapshot: Option<crate::pipeline_post_exec::PreExecutionSnapshot>,
    pub(super) sa_mode: bool,
}

impl SessionCompletionPlan {
    pub(super) fn merged_env_ref(&self) -> Option<&HashMap<String, String>> {
        (!self.merged_env.is_empty()).then_some(&self.merged_env)
    }
}

pub(super) async fn prepare_session_runtime(
    input: SessionRuntimeInput<'_>,
    session: &mut MetaSessionState,
    cleanup_guard: &mut Option<SessionCleanupGuard>,
) -> Result<SessionRuntimePlan> {
    let can_edit = input
        .config
        .is_none_or(|cfg| cfg.can_tool_edit_existing(input.executor.tool_name()));
    let can_write_new = input
        .config
        .is_none_or(|cfg| cfg.can_tool_write_new(input.executor.tool_name()));
    debug!(
        tool = %input.executor.tool_name(),
        can_edit,
        can_write_new,
        "Restriction flags resolved"
    );
    let raw_prompt = input.prompt.to_string();
    let prompt_caching_enabled = input
        .global_config
        .is_some_and(|cfg| cfg.experimental.enable_prompt_caching);
    let mut prompt_assembly = PromptAssembly::new(raw_prompt.clone(), prompt_caching_enabled);
    let state_dir_warning = match state_preflight::run(
        input.global_config,
        &session.meta_session_id,
        input.startup_env.session_id(),
        input.session_arg.is_none() || input.fresh_spawn_preflight_override,
    ) {
        Ok(warning) => warning,
        Err(err) => {
            return Err(persist_pipeline_pre_exec_failure(
                input.project_root,
                session,
                input.executor.tool_name(),
                err,
                cleanup_guard,
                None,
            ));
        }
    };
    if let Some(w) = state_dir_warning {
        prompt_assembly.prepend_dynamic(&w);
    }
    let is_first_turn = session
        .tools
        .get(input.executor.tool_name())
        .is_none_or(|ts| ts.provider_session_id.is_none());
    if is_first_turn {
        let first_turn_context = crate::pipeline::design_context::load_first_turn_context(
            &session.project_path,
            input.project_root,
            input.context_load_options,
            input
                .config
                .is_none_or(|cfg| cfg.session.resolved_plan_injection()),
        );
        prompt_assembly.add_first_turn_context(first_turn_context);
    }
    let is_review_or_debate = matches!(input.task_type, Some("review" | "debate"));
    if !is_review_or_debate {
        let memory_cfg = input
            .config
            .map(|cfg| &cfg.memory)
            .filter(|m| !m.is_default())
            .or_else(|| input.global_config.map(|cfg| &cfg.memory));
        session_exec_memory::append_memory_section(
            memory_cfg,
            input.memory_injection,
            raw_prompt.as_str(),
            input.memory_project_key,
            input.project_root,
            input.executor.tool_name(),
            prompt_assembly.dynamic_prompt_mut(),
        );
    }
    if !can_edit || !can_write_new {
        info!(
            tool = %input.executor.tool_name(),
            can_edit,
            can_write_new,
            "Applying filesystem restrictions via prompt injection"
        );
        prompt_assembly.add_restriction_instructions(
            input
                .executor
                .restriction_instructions(can_edit, can_write_new),
        );
    }
    let edit_guard = if !can_edit {
        crate::edit_restriction_guard::maybe_capture_tracked_file_guard(input.project_root)?
    } else {
        None
    };
    let commit_guard_enabled = matches!(input.task_type, Some("run"));
    let require_commit_on_mutation = commit_guard_enabled
        && input
            .config
            .is_some_and(|cfg| cfg.session.require_commit_on_mutation);
    let is_git = crate::run_cmd::is_git_worktree(input.project_root);
    let inside_git_worktree = commit_guard_enabled && is_git;
    let capture_snapshot = session_exec_audit::capture_git_workspace_snapshot_if_needed;
    let pre_run_workspace =
        capture_snapshot(is_git, input.project_root, require_commit_on_mutation);
    let tool_state =
        ensure_tool_state_initialized(session, input.executor, input.resolved_provider_session_id)
            .await?;
    let result_file_cleared =
        clear_expected_result_artifacts_for_prompt(input.prompt, input.session_dir);
    let execution_start_time = chrono::Utc::now();
    let sa_mode =
        std::env::var_os(crate::pipeline::prompt_guard::PROMPT_GUARD_CALLER_INJECTION_ENV)
            .is_some_and(|value| value == "true" || value == "1");
    let session_config = input.global_config.map(|gc| {
        let mcp_servers = resolve_mcp_servers(input.project_root, gc);
        if !mcp_servers.is_empty() {
            info!(
                count = mcp_servers.len(),
                servers = %mcp_servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "),
                "Injecting MCP servers into tool session"
            );
        }
        csa_executor::SessionConfig {
            mcp_servers,
            mcp_proxy_socket: gc.mcp_proxy_socket.clone(),
            ..Default::default()
        }
    });
    let mut merged_env =
        crate::pipeline_env::build_merged_env(crate::pipeline_env::MergedEnvRequest {
            extra_env: input.extra_env,
            config: input.config,
            global_config: input.global_config,
            tool_name: input.executor.tool_name(),
            current_depth: input.startup_env.current_depth(),
            pattern_internal: input.startup_env.pattern_internal(),
            allow_git_push: input.allow_git_push,
        });
    crate::pipeline_env::apply_task_target_dir_guards(
        input.task_type,
        input.executor.tool_name(),
        input.project_root,
        &mut merged_env,
    );
    let project_hook_overrides =
        crate::pipeline::session_hooks::build_project_hook_overrides(input.config, input.task_type);
    let hooks_config = load_hooks_config(
        csa_session::get_session_root(input.project_root)
            .ok()
            .map(|r| r.join("hooks.toml"))
            .as_deref(),
        global_hooks_path().as_deref(),
        project_hook_overrides.as_ref(),
    );
    let sessions_root = input
        .session_dir
        .parent()
        .unwrap_or(input.session_dir)
        .display()
        .to_string();
    let pre_run_vars = std::collections::HashMap::from([
        ("session_id".to_string(), session.meta_session_id.clone()),
        (
            "session_dir".to_string(),
            input.session_dir.display().to_string(),
        ),
        ("sessions_root".to_string(), sessions_root.clone()),
        ("tool".to_string(), input.executor.tool_name().to_string()),
        ("project_root".to_string(), session.project_path.clone()),
        ("CHANGED_PATHS".to_string(), "[]".to_string()),
        ("CHANGED_CRATES".to_string(), String::new()),
        ("CHANGED_CRATES_FLAGS".to_string(), String::new()),
    ]);
    run_pipeline_hook(HookEvent::PreRun, &hooks_config, &pre_run_vars)?;
    let new_file_guard = if !can_write_new {
        crate::edit_restriction_guard::maybe_capture_new_file_guard(input.project_root)?
    } else {
        None
    };
    session_exec_prompt_guard::inject_prompt_guards_if_needed(
        input.task_type,
        &hooks_config,
        session,
        input.executor,
        input.session_arg.is_some(),
        &mut prompt_assembly,
        input.startup_env.current_depth(),
    );
    let effective_prompt = session_exec_prompt_inject::finalize_effective_prompt(
        prompt_assembly,
        input.executor.tool_name(),
        input.task_type,
        is_first_turn,
        input.project_root,
        input.config,
    );
    let liveness_dead_seconds = resolve_liveness_dead_seconds(input.config);
    let mut execute_options = match crate::pipeline_sandbox::resolve_sandbox_options(
        input.config,
        input.executor.tool_name(),
        &session.meta_session_id,
        input.project_root,
        input.stream_mode,
        input.idle_timeout_seconds,
        liveness_dead_seconds,
        input.initial_response_timeout_seconds,
        input.no_fs_sandbox,
        input.readonly_project_root,
        input.extra_writable,
        input.extra_readable,
    ) {
        crate::pipeline_sandbox::SandboxResolution::Ok(opts) => *opts,
        crate::pipeline_sandbox::SandboxResolution::RequiredButUnavailable(msg) => {
            let err = anyhow::anyhow!(msg);
            return Err(persist_pipeline_pre_exec_failure(
                input.project_root,
                session,
                input.executor.tool_name(),
                err,
                cleanup_guard,
                None,
            ));
        }
    };
    crate::pipeline::ensure_tool_runtime_prerequisites(
        input.executor.tool_name(),
        crate::pipeline::resolved_filesystem_capability(&execute_options),
    )
    .await?;
    let spool_max_mb = input
        .config
        .map(|cfg| cfg.session.resolved_spool_max_mb())
        .unwrap_or((csa_process::DEFAULT_SPOOL_MAX_BYTES / (1024 * 1024)) as u32);
    let spool_max_bytes = u64::from(spool_max_mb).saturating_mul(1024 * 1024);
    let spool_keep_rotated = input
        .config
        .map(|cfg| cfg.session.resolved_spool_keep_rotated())
        .unwrap_or(csa_process::DEFAULT_SPOOL_KEEP_ROTATED);
    execute_options =
        execute_options.with_output_spool_rotation(spool_max_bytes, spool_keep_rotated);
    execute_options.output_spool = Some(input.session_dir.join("output.log"));
    let error_marker_scan_enabled = crate::error_marker_scan::resolve_error_marker_scan_enabled(
        input.error_marker_scan_override,
        input.startup_env.pattern_internal(),
        input.config.map(|cfg| cfg.resources.error_marker_scan),
    );
    execute_options = execute_options.with_error_marker_scan_enabled(error_marker_scan_enabled);
    let hook_bypass_scan_enabled = crate::run_cmd::resolve_hook_bypass_scan_enabled(
        input.cli_no_hook_bypass_scan,
        input.config.map(|cfg| cfg.resources.hook_bypass_scan),
    );
    execute_options = execute_options
        .with_subtree_pin(input.subtree_pin.cloned())
        .with_git_push_allowed(input.allow_git_push);
    apply_transport_failover_overrides(
        &mut execute_options,
        (!merged_env.is_empty()).then_some(&merged_env),
    );
    if let Some(pre_session_hook) = input.pre_session_hook {
        execute_options = execute_options.with_pre_session_hook(pre_session_hook);
    }
    crate::pipeline_sandbox::record_sandbox_telemetry(&execute_options, session);
    crate::pipeline_sandbox::maybe_inflate_balloon(
        input.tool.as_str(),
        input.project_root,
        &session.meta_session_id,
    );
    if let Err(err) =
        session_exec_metadata::persist_session_runtime_binary(input.session_dir, input.executor)
    {
        warn!(
            session = %session.meta_session_id,
            error = %err,
            "Failed to persist session runtime binary metadata"
        );
    }
    let pre_exec_snapshot = session_exec_audit::capture_pre_execution_snapshot(input.project_root);

    Ok(SessionRuntimePlan {
        effective_prompt,
        tool_state,
        execute_options,
        session_config,
        completion: SessionCompletionPlan {
            merged_env,
            hooks_config,
            sessions_root,
            edit_guard,
            new_file_guard,
            result_file_cleared,
            execution_start_time,
            commit_guard_enabled,
            require_commit_on_mutation,
            hook_bypass_scan_enabled,
            is_git,
            inside_git_worktree,
            pre_run_workspace,
            pre_exec_snapshot,
            sa_mode,
        },
    })
}
