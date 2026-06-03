use super::prompt_cache::PromptAssembly;
use super::result_contract::{
    clear_expected_result_artifacts_for_prompt, enforce_result_toml_path_contract,
};
use super::session_exec_failover::apply_transport_failover_overrides;
use super::{
    MemoryInjectionOptions, ParentSessionSource, SessionCreationMode, SessionExecutionResult,
    resolve_liveness_dead_seconds, resolve_mcp_servers, run_pipeline_hook,
};
use crate::pipeline_project_key::resolve_memory_project_key;
use crate::run_helpers::truncate_prompt;
use crate::session_guard::SessionCleanupGuard;
use crate::startup_env::StartupSubtreeEnv;
use anyhow::{Context, Result};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_executor::Executor;
use csa_hooks::{HookEvent, global_hooks_path, load_hooks_config};
use csa_lock::acquire_lock;
use csa_session::get_session_dir;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tracing::{debug, info, warn};
#[path = "pipeline_session_exec_api.rs"]
mod session_exec_api;
#[path = "pipeline_session_exec_audit.rs"]
mod session_exec_audit;
#[path = "pipeline_session_exec_bootstrap.rs"]
mod session_exec_bootstrap;
#[path = "pipeline_session_exec_memory.rs"]
mod session_exec_memory;
#[path = "pipeline_session_exec_metadata.rs"]
mod session_exec_metadata;
#[path = "pipeline_session_exec_pre_exec.rs"]
mod session_exec_pre_exec;
#[path = "pipeline_session_exec_prompt_guard.rs"]
mod session_exec_prompt_guard;
#[path = "pipeline_session_exec_tool_state.rs"]
mod session_exec_tool_state;
#[path = "pipeline_session_exec_write_guard.rs"]
mod session_exec_write_guard;
#[path = "pipeline_session_exec_state_preflight.rs"]
mod state_preflight;
use self::session_exec_pre_exec::{
    check_resources_before_spawn, persist_pipeline_pre_exec_failure,
    write_fatal_error_marker_sidecar,
};
use self::session_exec_tool_state::ensure_tool_state_initialized;
use self::session_exec_write_guard::apply_write_restriction_violations;
pub(crate) use session_exec_api::{execute_with_session, execute_with_session_and_meta};

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all, fields(tool = %tool, parent_session_source = ?parent_session_source))]
pub(crate) async fn execute_with_session_and_meta_with_parent_source(
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
    // Trusted CSA-decided subtree model pin (#1741), carried outside generic
    // `extra_env`. The executor applies it after env merges strip pin keys.
    // `None` means CSA did not pin; never source this from request/config env.
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
    parent_session_source: ParentSessionSource,
    session_creation_mode: SessionCreationMode,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
    cli_no_error_marker_scan: bool,
    cli_no_hook_bypass_scan: bool,
    startup_env: &StartupSubtreeEnv,
) -> Result<SessionExecutionResult> {
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
    // New-session cleanup guard: delete the orphan directory on pre-exec failure.
    let mut cleanup_guard = if session_arg.is_none() {
        Some(SessionCleanupGuard::new(session_dir.clone()))
    } else {
        None
    };
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
            ));
        }
    };
    let lock_reason = truncate_prompt(prompt, 80);
    let _lock = match acquire_lock(&session_dir, executor.tool_name(), &lock_reason) {
        Ok(lock) => lock,
        Err(e) => {
            let err = anyhow::anyhow!(e).context(format!(
                "Failed to acquire lock for session {}",
                session.meta_session_id
            ));
            return Err(persist_pipeline_pre_exec_failure(
                project_root,
                &mut session,
                executor.tool_name(),
                err,
                &mut cleanup_guard,
                None,
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
    )?;
    if let Some(ref budget) = session.token_budget {
        if budget.is_hard_exceeded() {
            warn!(
                session = %session.meta_session_id,
                used = budget.used,
                allocated = budget.allocated,
                pct = budget.usage_pct(),
                "Token budget hard threshold already exceeded — advisory only, execution continues"
            );
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
    let can_edit = config.is_none_or(|cfg| cfg.can_tool_edit_existing(executor.tool_name()));
    let can_write_new = config.is_none_or(|cfg| cfg.can_tool_write_new(executor.tool_name()));
    debug!(tool = %executor.tool_name(), can_edit, can_write_new, "Restriction flags resolved");
    let raw_prompt = prompt.to_string();
    let prompt_caching_enabled =
        global_config.is_some_and(|cfg| cfg.experimental.enable_prompt_caching);
    let mut prompt_assembly = PromptAssembly::new(raw_prompt.clone(), prompt_caching_enabled);
    let state_dir_warning = match state_preflight::run(
        global_config,
        &session.meta_session_id,
        startup_env.session_id(),
        session_arg.is_none() || fresh_spawn_preflight_override,
    ) {
        Ok(warning) => warning,
        Err(err) => {
            return Err(persist_pipeline_pre_exec_failure(
                project_root,
                &mut session,
                executor.tool_name(),
                err,
                &mut cleanup_guard,
                None,
            ));
        }
    };
    if let Some(w) = state_dir_warning {
        prompt_assembly.prepend_dynamic(&w);
    }
    let is_first_turn = session
        .tools
        .get(executor.tool_name())
        .is_none_or(|ts| ts.provider_session_id.is_none());
    if is_first_turn {
        let first_turn_context = super::design_context::load_first_turn_context(
            &session.project_path,
            project_root,
            context_load_options,
            config.is_none_or(|cfg| cfg.session.resolved_plan_injection()),
        );
        prompt_assembly.add_first_turn_context(first_turn_context);
    }
    let is_review_or_debate = matches!(task_type, Some("review" | "debate"));
    if !is_review_or_debate {
        let memory_cfg = config
            .map(|cfg| &cfg.memory)
            .filter(|m| !m.is_default())
            .or_else(|| global_config.map(|cfg| &cfg.memory));
        session_exec_memory::append_memory_section(
            memory_cfg,
            memory_injection,
            raw_prompt.as_str(),
            memory_project_key.as_deref(),
            project_root,
            executor.tool_name(),
            prompt_assembly.dynamic_prompt_mut(),
        );
    }
    if !can_edit || !can_write_new {
        info!(
            tool = %executor.tool_name(),
            can_edit,
            can_write_new,
            "Applying filesystem restrictions via prompt injection"
        );
        prompt_assembly.add_restriction_instructions(
            executor.restriction_instructions(can_edit, can_write_new),
        );
    }
    let edit_guard = if !can_edit {
        crate::edit_restriction_guard::maybe_capture_tracked_file_guard(project_root)?
    } else {
        None
    };
    // NOTE: new_file_guard captured AFTER PreRun hooks to avoid false positives.
    let commit_guard_enabled = matches!(task_type, Some("run"));
    let require_commit_on_mutation =
        commit_guard_enabled && config.is_some_and(|cfg| cfg.session.require_commit_on_mutation);
    // Check git status for both commit_guard and hook changed_paths variable.
    let is_git = crate::run_cmd::is_git_worktree(project_root);
    let inside_git_worktree = commit_guard_enabled && is_git;
    let capture_snapshot = session_exec_audit::capture_git_workspace_snapshot_if_needed;
    let pre_run_workspace = capture_snapshot(is_git, project_root, require_commit_on_mutation);
    let tool_state =
        ensure_tool_state_initialized(&mut session, executor, &resolved_provider_session_id)
            .await?;
    let result_file_cleared = clear_expected_result_artifacts_for_prompt(prompt, &session_dir);
    let execution_start_time = chrono::Utc::now();
    let session_config = global_config.map(|gc| {
        let mcp_servers = resolve_mcp_servers(project_root, gc);
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
    let mut merged_env = crate::pipeline_env::build_merged_env(
        extra_env,
        config,
        global_config,
        executor.tool_name(),
        startup_env.current_depth(),
    );
    crate::pipeline_env::apply_task_target_dir_guards(
        task_type,
        executor.tool_name(),
        project_root,
        &mut merged_env,
    );
    let merged_env_ref = (!merged_env.is_empty()).then_some(&merged_env);
    // Project [hooks] overrides take priority over hooks.toml entries.
    let project_hook_overrides =
        super::session_hooks::build_project_hook_overrides(config, task_type);
    // Load hooks config once, reused by PreRun, PostRun, and SessionComplete hooks.
    let hooks_config = load_hooks_config(
        csa_session::get_session_root(project_root)
            .ok()
            .map(|r| r.join("hooks.toml"))
            .as_deref(),
        global_hooks_path().as_deref(),
        project_hook_overrides.as_ref(),
    );
    let sessions_root = session_dir
        .parent()
        .unwrap_or(&session_dir)
        .display()
        .to_string();
    let pre_run_vars = std::collections::HashMap::from([
        ("session_id".to_string(), session.meta_session_id.clone()),
        ("session_dir".to_string(), session_dir.display().to_string()),
        ("sessions_root".to_string(), sessions_root.clone()),
        ("tool".to_string(), executor.tool_name().to_string()),
        ("project_root".to_string(), session.project_path.clone()),
        // Empty at PreRun; populated at PostRun after git diff.
        ("CHANGED_PATHS".to_string(), "[]".to_string()),
        ("CHANGED_CRATES".to_string(), String::new()),
        ("CHANGED_CRATES_FLAGS".to_string(), String::new()),
    ]);
    run_pipeline_hook(HookEvent::PreRun, &hooks_config, &pre_run_vars)?;
    // New-file guard captured AFTER PreRun hooks (baseline includes hook-created files).
    let new_file_guard = if !can_write_new {
        crate::edit_restriction_guard::maybe_capture_new_file_guard(project_root)?
    } else {
        None
    };
    session_exec_prompt_guard::inject_prompt_guards_if_needed(
        task_type,
        &hooks_config,
        &session,
        executor,
        session_arg.is_some(),
        &mut prompt_assembly,
        startup_env.current_depth(),
    );
    // Inject structured output section markers when enabled in config.
    let structured_output_enabled = config.is_none_or(|cfg| cfg.session.structured_output);
    if let Some(instructions) =
        csa_executor::structured_output_instructions(structured_output_enabled)
    {
        info!("Injecting structured output instructions into prompt");
        prompt_assembly.add_static_or_append_dynamic(instructions);
    }
    let effective_prompt = prompt_assembly.finish();
    // Resolve sandbox configuration from project config and enforcement mode.
    let liveness_dead_seconds = resolve_liveness_dead_seconds(config);
    let mut execute_options = match crate::pipeline_sandbox::resolve_sandbox_options(
        config,
        executor.tool_name(),
        &session.meta_session_id,
        project_root,
        stream_mode,
        idle_timeout_seconds,
        liveness_dead_seconds,
        initial_response_timeout_seconds,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
        extra_readable,
    ) {
        crate::pipeline_sandbox::SandboxResolution::Ok(opts) => *opts,
        crate::pipeline_sandbox::SandboxResolution::RequiredButUnavailable(msg) => {
            let err = anyhow::anyhow!(msg);
            return Err(persist_pipeline_pre_exec_failure(
                project_root,
                &mut session,
                executor.tool_name(),
                err,
                &mut cleanup_guard,
                None,
            ));
        }
    };
    let spool_max_mb = config
        .map(|cfg| cfg.session.resolved_spool_max_mb())
        .unwrap_or((csa_process::DEFAULT_SPOOL_MAX_BYTES / (1024 * 1024)) as u32);
    let spool_max_bytes = u64::from(spool_max_mb).saturating_mul(1024 * 1024);
    let spool_keep_rotated = config
        .map(|cfg| cfg.session.resolved_spool_keep_rotated())
        .unwrap_or(csa_process::DEFAULT_SPOOL_KEEP_ROTATED);
    execute_options =
        execute_options.with_output_spool_rotation(spool_max_bytes, spool_keep_rotated);
    execute_options.output_spool = Some(session_dir.join("output.log"));
    // Resolve the #1652 fatal-error-marker scan opt-out (#1745). Precedence:
    // CLI `--no-error-marker-scan` (forces OFF) > config `[resources].error_marker_scan`
    // (default-true) > built-in default (ON). Only the marker-based fatal classification
    // is affected; idle/wall-clock timeouts still apply.
    let error_marker_scan_enabled =
        !cli_no_error_marker_scan && config.is_none_or(|cfg| cfg.resources.error_marker_scan);
    execute_options = execute_options.with_error_marker_scan_enabled(error_marker_scan_enabled);
    let hook_bypass_scan_enabled = crate::run_cmd::resolve_hook_bypass_scan_enabled(
        cli_no_hook_bypass_scan,
        config.map(|cfg| cfg.resources.hook_bypass_scan),
    );
    // #1741: carry CSA's trusted subtree pin via the typed ExecuteOptions
    // channel (never via the generic env map), so it is the sole writer of the
    // pin keys at the spawn boundary.
    execute_options = execute_options.with_subtree_pin(subtree_pin.cloned());
    apply_transport_failover_overrides(&mut execute_options, merged_env_ref);
    if let Some(pre_session_hook) = pre_session_hook {
        execute_options = execute_options.with_pre_session_hook(pre_session_hook);
    }
    crate::pipeline_sandbox::record_sandbox_telemetry(&execute_options, &mut session);
    crate::pipeline_sandbox::maybe_inflate_balloon(tool.as_str());
    if let Err(err) = session_exec_metadata::persist_session_runtime_binary(&session_dir, executor)
    {
        warn!(
            session = %session.meta_session_id,
            error = %err,
            "Failed to persist session runtime binary metadata"
        );
    }
    let pre_exec_snapshot = session_exec_audit::capture_pre_execution_snapshot(project_root);
    let transport_result = crate::pipeline_execute::execute_transport_with_signal(
        executor,
        &effective_prompt,
        tool_state.as_ref(),
        &session,
        merged_env_ref,
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
    let provider_session_id =
        csa_executor::extract_session_id_from_transport(tool, &transport_result);
    let events_count = transport_result
        .metadata
        .total_events_count
        .max(transport_result.events.len()) as u64;
    let execute_events_observed = crate::run_cmd::execute_tool_calls_observed(
        &transport_result.metadata,
        &transport_result.events,
    );
    let mut executed_shell_commands = crate::run_cmd::extract_executed_shell_commands(
        &transport_result.metadata,
        &transport_result.events,
    );
    // Re-inject --no-verify commit if evicted from ring buffer.
    if transport_result.metadata.has_no_verify_commit
        && crate::run_cmd::detect_no_verify_commit_commands(&executed_shell_commands).is_empty()
    {
        executed_shell_commands.push("git commit --no-verify".to_string());
    }
    let transcript_artifacts =
        crate::pipeline_transcript::persist_if_enabled(config, &session_dir, &transport_result);
    let mut result = transport_result.execution;
    // Best-effort EACCES diagnostic when filesystem sandbox is active.
    crate::pipeline_sandbox::check_sandbox_permission_errors(
        &result.stderr_output,
        session.sandbox_info.as_ref(),
    );
    enforce_result_toml_path_contract(
        prompt,
        &effective_prompt,
        &session_dir,
        result_file_cleared,
        &mut result,
    );
    apply_write_restriction_violations(edit_guard, new_file_guard, executor, &mut result)?;
    if result.exit_code != 0 {
        crate::error_hints::append_sandbox_fs_denial_hint(
            &mut result.stderr_output,
            &result.output,
            crate::pipeline_sandbox::filesystem_sandbox_active(session.sandbox_info.as_ref()),
            &session.meta_session_id,
        );
    }
    // Post-run git snapshot for commit guard + changed_paths hook vars.
    let post_run_workspace = capture_snapshot(is_git, project_root, require_commit_on_mutation);
    let pre_fingerprints = pre_run_workspace
        .as_ref()
        .map(session_exec_audit::snapshot_to_fingerprints);
    let post_fingerprints = post_run_workspace
        .as_ref()
        .map(session_exec_audit::snapshot_to_fingerprints);
    let changed_paths = crate::pipeline::changed_paths::compute_changed_paths(
        pre_run_workspace.as_ref().map(|s| s.status.as_str()),
        post_run_workspace.as_ref().map(|s| s.status.as_str()),
        pre_fingerprints.as_ref(),
        post_fingerprints.as_ref(),
    );
    let snapshots_available = pre_run_workspace.is_some() && post_run_workspace.is_some();
    if commit_guard_enabled {
        let commit_guard = crate::run_cmd::evaluate_post_run_commit_guard(
            pre_run_workspace.as_ref(),
            post_run_workspace.as_ref(),
        );
        let policy_evaluation_failed = require_commit_on_mutation
            && (!inside_git_worktree
                || pre_run_workspace.is_none()
                || post_run_workspace.is_none());
        crate::run_cmd::apply_post_session_commit_policies(
            &mut result,
            crate::run_cmd::PostSessionCommitPolicyArgs {
                output_format: &output_format,
                prompt,
                require_commit_on_mutation,
                commit_guard: commit_guard.as_ref(),
                policy_evaluation_failed,
                hook_bypass_scan_enabled,
                executed_shell_commands: &executed_shell_commands,
                merged_env_ref,
                execute_events_observed,
            },
        );
    }
    let sa_mode = std::env::var(crate::pipeline::prompt_guard::PROMPT_GUARD_CALLER_INJECTION_ENV)
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"))
        .unwrap_or(false);
    let post_ctx = crate::pipeline_post_exec::PostExecContext {
        executor,
        prompt,
        effective_prompt: &effective_prompt,
        task_type,
        readonly_project_root,
        project_root,
        config,
        global_config,
        session_dir,
        sessions_root,
        execution_start_time,
        hooks_config: &hooks_config,
        memory_project_key,
        provider_session_id: provider_session_id.clone(),
        events_count,
        transcript_artifacts,
        changed_paths: changed_paths.clone(),
        pre_exec_snapshot,
        has_tool_calls: transport_result.metadata.has_tool_calls
            || transport_result.metadata.has_execute_tool_calls,
        turn_count: transport_result.metadata.turn_count,
        output_tokens: transport_result.metadata.output_tokens,
        sa_mode,
    };
    if let Err(err) =
        crate::pipeline_post_exec::process_execution_result(post_ctx, &mut session, &mut result)
            .await
    {
        crate::pipeline_post_exec::ensure_terminal_result_on_post_exec_error(
            project_root,
            &mut session,
            executor.tool_name(),
            execution_start_time,
            &err,
        );
        return Err(err).with_context(|| format!("meta_session_id={}", session.meta_session_id));
    }
    Ok(SessionExecutionResult {
        execution: result,
        meta_session_id: session.meta_session_id.clone(),
        provider_session_id,
        changed_paths: snapshots_available.then_some(changed_paths),
    })
}
