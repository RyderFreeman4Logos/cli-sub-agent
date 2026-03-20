//! Session-bound execution pipeline: resolve-or-create session, run tool,
//! post-process results.
//!
//! Three entry points with increasing control surface:
//! - [`execute_with_session`] — simplest, returns only the execution result.
//! - [`execute_with_session_and_meta`] — also returns meta session ID and
//!   provider session ID.
//! - [`execute_with_session_and_meta_with_parent_source`] — additionally
//!   controls how the parent session ID is resolved.

use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;
use tracing::{debug, info, warn};

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

use crate::memory_capture;
use crate::pipeline_project_key::resolve_memory_project_key;
use crate::run_helpers::truncate_prompt;
use crate::session_guard::{SessionCleanupGuard, write_pre_exec_error_result};

use super::prompt_guard::emit_prompt_guard_to_caller;
use super::result_contract::{clear_expected_result_toml, enforce_result_toml_path_contract};
use super::{
    MemoryInjectionOptions, ParentSessionSource, SessionExecutionResult,
    resolve_liveness_dead_seconds, resolve_mcp_servers, run_pipeline_hook,
};

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
) -> Result<SessionExecutionResult> {
    // Check for parent session violation: a child process must not operate on its own session
    if let Some(ref session_id) = session_arg
        && let Ok(env_session) = std::env::var("CSA_SESSION_ID")
        && env_session == *session_id
    {
        return Err(csa_core::error::AppError::ParentSessionViolation.into());
    }
    let memory_project_key = resolve_memory_project_key(project_root);

    // Resolve or create session
    let mut resolved_provider_session_id: Option<String> = None;
    let mut session = if let Some(ref session_id) = session_arg {
        let resolution =
            csa_session::resolve_resume_session(project_root, session_id, tool.as_str())?;
        resolved_provider_session_id = resolution.provider_session_id;
        if resolved_provider_session_id.is_some() {
            info!(
                session = %resolution.meta_session_id,
                tool = %executor.tool_name(),
                "Resolved provider session ID from state.toml"
            );
        }
        csa_session::load_session(project_root, &resolution.meta_session_id)?
    } else {
        // Auto-generate description from prompt when not provided
        let effective_description = description.or_else(|| Some(truncate_prompt(prompt, 80)));
        let parent_id = match parent_session_source {
            ParentSessionSource::ExplicitOrEnv => {
                parent.or_else(|| std::env::var("CSA_SESSION_ID").ok())
            }
            ParentSessionSource::ExplicitOnly => parent,
        };
        let mut new_session = create_session(
            project_root,
            effective_description.as_deref(),
            parent_id.as_deref(),
            Some(tool.as_str()),
        )?;
        // Populate task context on newly created sessions
        new_session.task_context = csa_session::TaskContext {
            task_type: task_type.map(|s| s.to_string()),
            tier_name: tier_name.map(|s| s.to_string()),
        };
        // Initialize token budget from tier config (if configured).
        // TokenBudget is created when either token_budget or max_turns is set.
        if let (Some(cfg), Some(tier)) = (config, tier_name)
            && let Some(tier_cfg) = cfg.tiers.get(tier)
            && (tier_cfg.token_budget.is_some() || tier_cfg.max_turns.is_some())
        {
            let allocated = tier_cfg.token_budget.unwrap_or(u64::MAX);
            let mut budget = csa_session::state::TokenBudget::new(allocated);
            budget.max_turns = tier_cfg.max_turns;
            new_session.token_budget = Some(budget);
            info!(
                session = %new_session.meta_session_id,
                allocated = ?tier_cfg.token_budget,
                max_turns = ?tier_cfg.max_turns,
                "Initialized token budget from tier config"
            );
        }
        new_session
    };

    // Resuming an Available session re-activates it for execution.
    if session_arg.is_some() && session.phase == csa_session::SessionPhase::Available {
        match session.apply_phase_event(csa_session::PhaseEvent::Resumed) {
            Ok(()) => {
                info!(
                    session = %session.meta_session_id,
                    "Session resumed and marked Active"
                );
            }
            Err(e) => {
                warn!(
                    session = %session.meta_session_id,
                    error = %e,
                    "Skipping phase transition on resume"
                );
            }
        }
    }

    let session_dir = get_session_dir(project_root, &session.meta_session_id)?;

    // Arm cleanup guard for new sessions only (not resumed ones).
    // If any pre-execution step fails, the guard deletes the orphan directory.
    let mut cleanup_guard = if session_arg.is_none() {
        Some(SessionCleanupGuard::new(session_dir.clone()))
    } else {
        None
    };

    // Create session log writer
    let (_log_writer, _log_guard) = match csa_executor::create_session_log_writer(&session_dir) {
        Ok(pair) => pair,
        Err(e) => {
            let err = anyhow::anyhow!(e).context("Failed to create session log writer");
            write_pre_exec_error_result(
                project_root,
                &session.meta_session_id,
                executor.tool_name(),
                &err,
            );
            if let Some(ref mut cg) = cleanup_guard {
                cg.defuse();
            }
            return Err(err);
        }
    };

    // Acquire lock with truncated prompt as reason
    let lock_reason = truncate_prompt(prompt, 80);
    let _lock = match acquire_lock(&session_dir, executor.tool_name(), &lock_reason) {
        Ok(lock) => lock,
        Err(e) => {
            let err = anyhow::anyhow!(e).context(format!(
                "Failed to acquire lock for session {}",
                session.meta_session_id
            ));
            write_pre_exec_error_result(
                project_root,
                &session.meta_session_id,
                executor.tool_name(),
                &err,
            );
            if let Some(ref mut cg) = cleanup_guard {
                cg.defuse();
            }
            return Err(err);
        }
    };

    // Resource guard
    let mut resource_guard = if let Some(cfg) = config {
        let limits = ResourceLimits {
            min_free_memory_mb: cfg.resources.min_free_memory_mb,
        };
        Some(ResourceGuard::new(limits))
    } else {
        None
    };

    // Check resource availability
    if let Some(ref mut guard) = resource_guard
        && let Err(e) = guard.check_availability(executor.tool_name())
    {
        write_pre_exec_error_result(
            project_root,
            &session.meta_session_id,
            executor.tool_name(),
            &e,
        );
        if let Some(ref mut cg) = cleanup_guard {
            cg.defuse();
        }
        return Err(e);
    }

    // Token budget is observability-only (never a kill gate).
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
    let mut effective_prompt = raw_prompt.clone();

    // Auto-inject project context (CLAUDE.md, AGENTS.md) on first turn only.
    // Session resumes already have context loaded in the tool's conversation.
    let is_first_turn = session
        .tools
        .get(executor.tool_name())
        .is_none_or(|ts| ts.provider_session_id.is_none());
    if is_first_turn {
        let context_load_options = context_load_options.cloned().unwrap_or_default();
        let context_files = csa_executor::load_project_context(
            Path::new(&session.project_path),
            &context_load_options,
        );
        if !context_files.is_empty() {
            let context_block = csa_executor::format_context_for_prompt(&context_files);
            info!(
                file_count = context_files.len(),
                bytes = context_block.len(),
                "Injecting project context into prompt"
            );
            effective_prompt = format!("{context_block}{effective_prompt}");
        }
    }

    // Inject memory after context, before restrictions.
    let is_review_or_debate = matches!(task_type, Some("review" | "debate"));
    if !is_review_or_debate {
        let memory_cfg = config
            .map(|cfg| &cfg.memory)
            .filter(|m| !m.is_default())
            .or_else(|| global_config.map(|cfg| &cfg.memory));
        let memory_disabled =
            memory_injection.is_none() || memory_injection.is_some_and(|opts| opts.disabled);
        if let Some(memory_cfg) = memory_cfg
            && memory_cfg.inject
            && !memory_disabled
        {
            let memory_query = memory_injection
                .and_then(|opts| opts.query_override.as_deref())
                .unwrap_or(raw_prompt.as_str());
            if let Some(memory_section) = memory_capture::build_memory_section(
                memory_cfg,
                memory_query,
                memory_project_key.as_deref(),
            ) {
                info!(
                    bytes = memory_section.len(),
                    "Injecting memory context into prompt"
                );
                effective_prompt.push_str(&memory_section);
            }
        }
    }

    // Apply restrictions after context and memory injection.
    if !can_edit || !can_write_new {
        info!(
            tool = %executor.tool_name(),
            can_edit,
            can_write_new,
            "Applying filesystem restrictions via prompt injection"
        );
        effective_prompt = executor.apply_restrictions(&effective_prompt, can_edit, can_write_new);
    }
    let edit_guard = if !can_edit {
        crate::edit_restriction_guard::maybe_capture_tracked_file_guard(project_root)?
    } else {
        None
    };
    // NOTE: new_file_guard is captured AFTER PreRun hooks (below) to avoid
    // false positives from hook-created files. See the edit_guard capture here
    // for tracked-file protection (hooks should not modify tracked files).

    let commit_guard_enabled = matches!(task_type, Some("run"));
    let require_commit_on_mutation =
        commit_guard_enabled && config.is_some_and(|cfg| cfg.session.require_commit_on_mutation);
    let inside_git_worktree = commit_guard_enabled && crate::run_cmd::is_git_worktree(project_root);
    let pre_run_workspace = if inside_git_worktree {
        crate::run_cmd::capture_git_workspace_snapshot(project_root, require_commit_on_mutation)
    } else {
        None
    };

    // Resolve tool state for session resume.
    let tool_state = session
        .tools
        .get(executor.tool_name())
        .cloned()
        .or_else(|| {
            resolved_provider_session_id
                .as_ref()
                .map(|provider_session_id| ToolState {
                    provider_session_id: Some(provider_session_id.clone()),
                    last_action_summary: String::new(),
                    last_exit_code: 0,
                    updated_at: chrono::Utc::now(),
                    token_usage: None,
                })
        });

    let result_file_cleared = clear_expected_result_toml(&session_dir.join("result.toml"));

    // Record execution start time before spawning.
    let execution_start_time = chrono::Utc::now();

    // Build session config with MCP servers (if global config provided).
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

    // Inject CSA_SUPPRESS_NOTIFY to skip desktop notifications in non-interactive mode.
    let merged_env = crate::pipeline_env::build_merged_env(extra_env, config, executor.tool_name());
    let merged_env_ref = if merged_env.is_empty() {
        None
    } else {
        Some(&merged_env)
    };

    // Build runtime overrides from .csa/config.toml [hooks] section.
    // These take PRIORITY over hooks.toml PreRun/PostRun entries.
    let project_hook_overrides = config.filter(|c| !c.hooks.is_default()).map(|c| {
        let mut overrides = std::collections::HashMap::new();
        if let Some(ref cmd) = c.hooks.pre_run {
            overrides.insert(
                "pre_run".to_string(),
                csa_hooks::HookConfig {
                    enabled: true,
                    command: Some(cmd.clone()),
                    timeout_secs: c.hooks.timeout_secs,
                    fail_policy: csa_hooks::FailPolicy::default(),
                    waivers: Vec::new(),
                },
            );
        }
        if let Some(ref cmd) = c.hooks.post_run {
            overrides.insert(
                "post_run".to_string(),
                csa_hooks::HookConfig {
                    enabled: true,
                    command: Some(cmd.clone()),
                    timeout_secs: c.hooks.timeout_secs,
                    fail_policy: csa_hooks::FailPolicy::default(),
                    waivers: Vec::new(),
                },
            );
        }
        overrides
    });

    // Load hooks config once, reused by PreRun, PostRun, and SessionComplete hooks.
    let hooks_config = load_hooks_config(
        csa_session::get_session_root(project_root)
            .ok()
            .map(|r| r.join("hooks.toml"))
            .as_deref(),
        global_hooks_path().as_deref(),
        project_hook_overrides.as_ref(),
    );
    // PreRun hook: fires before tool execution starts.
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
    ]);
    run_pipeline_hook(HookEvent::PreRun, &hooks_config, &pre_run_vars)?;

    // Capture new-file guard AFTER PreRun hooks so hook-created files are
    // included in the baseline and not flagged as tool violations.
    let new_file_guard = if !can_write_new {
        crate::edit_restriction_guard::maybe_capture_new_file_guard(project_root)?
    } else {
        None
    };

    // Run prompt guards: append reminders to effective_prompt (strongest influence at end).
    if !hooks_config.prompt_guard.is_empty() {
        let guard_context = GuardContext {
            project_root: session.project_path.clone(),
            session_id: session.meta_session_id.clone(),
            tool: executor.tool_name().to_string(),
            is_resume: session_arg.is_some(),
            cwd: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        };
        let guard_results = run_prompt_guards(&hooks_config.prompt_guard, &guard_context);
        if let Some(guard_block) = format_guard_output(&guard_results) {
            info!(
                guard_count = guard_results.len(),
                bytes = guard_block.len(),
                "Injecting prompt guard output into effective prompt"
            );
            emit_prompt_guard_to_caller(&guard_block, guard_results.len());
            effective_prompt = format!("{effective_prompt}\n\n{guard_block}");
        }
    }

    // Inject structured output section markers when enabled in config.
    let structured_output_enabled = config.is_none_or(|cfg| cfg.session.structured_output);
    if let Some(instructions) =
        csa_executor::structured_output_instructions(structured_output_enabled)
    {
        info!("Injecting structured output instructions into prompt");
        effective_prompt.push_str(instructions);
    }

    // Resolve sandbox configuration from project config and enforcement mode.
    let liveness_dead_seconds = resolve_liveness_dead_seconds(config);
    let mut execute_options = match crate::pipeline_sandbox::resolve_sandbox_options(
        config,
        executor.tool_name(),
        &session.meta_session_id,
        stream_mode,
        idle_timeout_seconds,
        liveness_dead_seconds,
        initial_response_timeout_seconds,
        false, // no_fs_sandbox: default — CLI flag is handled upstream
    ) {
        crate::pipeline_sandbox::SandboxResolution::Ok(opts) => *opts,
        crate::pipeline_sandbox::SandboxResolution::RequiredButUnavailable(msg) => {
            let err = anyhow::anyhow!(msg);
            write_pre_exec_error_result(
                project_root,
                &session.meta_session_id,
                executor.tool_name(),
                &err,
            );
            if let Some(ref mut cg) = cleanup_guard {
                cg.defuse();
            }
            return Err(err);
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

    // Record sandbox telemetry in session state (first turn only).
    crate::pipeline_sandbox::record_sandbox_telemetry(&execute_options, &mut session);

    // Memory balloon: pre-warm swap for heavyweight tools.
    crate::pipeline_sandbox::maybe_inflate_balloon(tool.as_str());

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

    // Tool execution completed — defuse cleanup guard (preserve artifacts on later errors).
    if let Some(ref mut guard) = cleanup_guard {
        guard.defuse();
    }

    // Extract provider session ID from transport metadata or fallback output parsing.
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
    // If the streaming metadata caught a --no-verify commit that was
    // subsequently evicted from the bounded command ring buffer, re-inject
    // it so that the post-run policy check can still block it.
    if transport_result.metadata.has_no_verify_commit
        && !executed_shell_commands
            .iter()
            .any(|c| c.contains("--no-verify") || c.contains("-n"))
    {
        executed_shell_commands.push("git commit --no-verify".to_string());
    }
    let transcript_artifacts =
        crate::pipeline_transcript::persist_if_enabled(config, &session_dir, &transport_result);
    let mut result = transport_result.execution;
    enforce_result_toml_path_contract(
        prompt,
        &effective_prompt,
        &session_dir,
        result_file_cleared,
        &mut result,
    );
    if let Some(guard) = edit_guard
        && let Some(violation) = guard.enforce_and_restore()?
    {
        let violation_summary = violation.summary();
        let violation_details = violation.detail_message();
        let previous_summary = result.summary.clone();
        warn!(
            tool = %executor.tool_name(),
            modified = violation.modified_paths.len(),
            restored = violation.restored_paths.len(),
            "Detected and reverted edits to existing tracked files under edit restriction"
        );
        if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        if !previous_summary.trim().is_empty() {
            result.stderr_output.push_str(&format!(
                "Original summary before restriction guard: {previous_summary}\n"
            ));
        }
        result.stderr_output.push_str(&violation_details);
        if !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        result.summary = violation_summary;
        result.exit_code = 1;
    }
    if let Some(guard) = new_file_guard
        && let Some(violation) = guard.enforce_and_remove()?
    {
        let violation_summary = violation.summary();
        let violation_details = violation.detail_message();
        warn!(
            tool = %executor.tool_name(),
            new_files = violation.new_paths.len(),
            removed = violation.removed_paths.len(),
            "Detected and removed new files created under write restriction"
        );
        if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        result.stderr_output.push_str(&violation_details);
        if !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        // Only override summary/exit if edit guard didn't already fail.
        if result.exit_code == 0 {
            result.summary = violation_summary;
        }
        result.exit_code = 1;
    }
    if commit_guard_enabled {
        let post_run_workspace = if inside_git_worktree {
            crate::run_cmd::capture_git_workspace_snapshot(project_root, require_commit_on_mutation)
        } else {
            None
        };
        let commit_guard = crate::run_cmd::evaluate_post_run_commit_guard(
            pre_run_workspace.as_ref(),
            post_run_workspace.as_ref(),
        );
        let policy_evaluation_failed = require_commit_on_mutation
            && (!inside_git_worktree
                || pre_run_workspace.is_none()
                || post_run_workspace.is_none());
        crate::run_cmd::apply_post_run_commit_policy(
            &mut result,
            &output_format,
            require_commit_on_mutation,
            commit_guard.as_ref(),
        );
        crate::run_cmd::apply_unverifiable_commit_policy(
            &mut result,
            &output_format,
            policy_evaluation_failed,
        );
        crate::run_cmd::apply_no_verify_commit_policy(
            &mut result,
            &output_format,
            prompt,
            &executed_shell_commands,
            execute_events_observed,
        );
    }

    // Delegate post-execution processing (state updates, persistence, hooks, memory).
    let post_ctx = crate::pipeline_post_exec::PostExecContext {
        executor,
        prompt,
        effective_prompt: &effective_prompt,
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
    };
    if let Err(err) =
        crate::pipeline_post_exec::process_execution_result(post_ctx, &mut session, &result).await
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
    })
}
