//! Shared execution pipeline functions for CSA command handlers.
//!
//! This module extracts common patterns from run, review, and debate handlers:
//! - Config loading and recursion depth validation
//! - Executor building and tool installation checks
//! - Global slot acquisition with concurrency limits

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::{error, info, warn};

use csa_config::{GlobalConfig, McpRegistry, ProjectConfig};
use csa_core::types::ToolName;
use csa_executor::{AcpMcpServerConfig, Executor, SessionConfig, create_session_log_writer};
use csa_hooks::{
    GuardContext, HookEvent, format_guard_output, global_hooks_path, load_hooks_config,
    run_hooks_for_event, run_prompt_guards,
};
use csa_lock::acquire_lock;
use csa_process::{ExecutionResult, check_tool_installed};
use csa_resource::{ResourceGuard, ResourceLimits};
use csa_session::{ToolState, create_session, get_session_dir};

use crate::memory_capture;
use crate::pipeline_project_key::resolve_memory_project_key;
use crate::run_helpers::truncate_prompt;
use crate::session_guard::{SessionCleanupGuard, write_pre_exec_error_result};
#[path = "pipeline_prompt_guard.rs"]
mod prompt_guard;
use prompt_guard::emit_prompt_guard_to_caller;

pub(crate) const DEFAULT_IDLE_TIMEOUT_SECONDS: u64 = 120;
pub(crate) const DEFAULT_LIVENESS_DEAD_SECONDS: u64 = csa_process::DEFAULT_LIVENESS_DEAD_SECS;

pub(crate) fn resolve_idle_timeout_seconds(
    config: Option<&ProjectConfig>,
    cli_override: Option<u64>,
) -> u64 {
    cli_override
        .or_else(|| config.map(|cfg| cfg.resources.idle_timeout_seconds))
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_SECONDS)
}

pub(crate) fn resolve_liveness_dead_seconds(config: Option<&ProjectConfig>) -> u64 {
    config
        .and_then(|cfg| cfg.resources.liveness_dead_seconds)
        .unwrap_or(DEFAULT_LIVENESS_DEAD_SECONDS)
}

pub(crate) fn context_load_options_with_skips(
    skip_files: &[String],
) -> Option<csa_executor::ContextLoadOptions> {
    if skip_files.is_empty() {
        None
    } else {
        Some(csa_executor::ContextLoadOptions {
            skip_files: skip_files.to_vec(),
            ..Default::default()
        })
    }
}

/// Load ProjectConfig and GlobalConfig, validate recursion depth.
///
/// Returns `Some((project_config, global_config))` on success.
/// Returns `Ok(None)` if recursion depth exceeded (caller should exit with code 1).
/// Returns `Err` for config loading/parsing failures (caller should propagate).
pub(crate) fn load_and_validate(
    project_root: &Path,
    current_depth: u32,
) -> Result<Option<(Option<ProjectConfig>, GlobalConfig)>> {
    let config = ProjectConfig::load(project_root)?;

    let max_depth = config
        .as_ref()
        .map(|c| c.project.max_recursion_depth)
        .unwrap_or(5u32);

    if current_depth > max_depth {
        error!(
            "Max recursion depth ({}) exceeded. Current: {}. Do it yourself.",
            max_depth, current_depth
        );
        return Ok(None);
    }

    let global_config = GlobalConfig::load()?;
    Ok(Some((config, global_config)))
}

/// Load and merge MCP server registries from global + project config.
///
/// Returns a merged list of [`AcpMcpServerConfig`] ready for transport injection.
/// Global servers are included unless overridden by a project server with the same name.
pub(crate) fn resolve_mcp_servers(
    project_root: &Path,
    global_config: &GlobalConfig,
) -> Vec<AcpMcpServerConfig> {
    let global_servers = global_config.mcp_servers();

    let project_registry = match McpRegistry::load(project_root) {
        Ok(Some(registry)) => registry,
        Ok(None) => {
            // No project MCP config; use global servers only
            return global_servers
                .iter()
                .filter_map(config_to_acp_mcp)
                .collect();
        }
        Err(e) => {
            warn!("Failed to load project MCP registry: {e}");
            return global_servers
                .iter()
                .filter_map(config_to_acp_mcp)
                .collect();
        }
    };

    let merged = McpRegistry::merge(global_servers, &project_registry);
    merged
        .servers
        .iter()
        .filter_map(config_to_acp_mcp)
        .collect()
}

/// Convert `csa_config::McpServerConfig` to [`AcpMcpServerConfig`].
///
/// Only stdio transport servers can be injected into ACP sessions (tools
/// launch subprocesses directly). Remote transport servers are filtered out.
fn config_to_acp_mcp(cfg: &csa_config::McpServerConfig) -> Option<AcpMcpServerConfig> {
    match &cfg.transport {
        csa_config::McpTransport::Stdio {
            command, args, env, ..
        } => Some(AcpMcpServerConfig {
            name: cfg.name.clone(),
            command: command.clone(),
            args: args.clone(),
            env: env.clone(),
        }),
        _ => None,
    }
}

/// References to project and global config for executor building.
pub(crate) struct ConfigRefs<'a> {
    pub project: Option<&'a ProjectConfig>,
    pub global: Option<&'a GlobalConfig>,
}

/// Build executor and validate tool is installed and enabled.
///
/// Returns Executor on success.
/// Returns error if tool not installed or disabled in config.
///
/// When `enforce_tier` is `false`, tier whitelist and model-name checks are
/// skipped. Review and debate commands use this because they select tools for
/// heterogeneous evaluation, not for tier-controlled execution.
///
/// If the tool has a `thinking_lock` in project or global config, the locked
/// value silently overrides any CLI-provided thinking budget.
pub(crate) async fn build_and_validate_executor(
    tool: &ToolName,
    model_spec: Option<&str>,
    model: Option<&str>,
    thinking_budget: Option<&str>,
    configs: ConfigRefs<'_>,
    enforce_tier: bool,
    force_override_user_config: bool,
) -> Result<Executor> {
    let mut executor = crate::run_helpers::build_executor(
        tool,
        model_spec,
        model,
        thinking_budget,
        configs.project,
    )?;

    // Apply thinking lock: project config takes precedence over global.
    // When set, silently overrides any CLI-provided thinking budget (including
    // the one embedded in --model-spec).
    let tool_str = tool.as_str();
    let lock_from_project = configs.project.and_then(|c| c.thinking_lock(tool_str));
    let lock_from_global = configs.global.and_then(|g| g.thinking_lock(tool_str));
    if let Some(lock_str) = lock_from_project.or(lock_from_global) {
        let locked_budget = csa_executor::ThinkingBudget::parse(lock_str)?;
        executor.override_thinking_budget(locked_budget);
    }

    // Defense-in-depth: enforce tool enablement from user config
    if let Some(cfg) = configs.project {
        cfg.enforce_tool_enabled(executor.tool_name(), force_override_user_config)?;

        if enforce_tier {
            // Defense-in-depth: enforce tier whitelist at execution boundary
            cfg.enforce_tier_whitelist(executor.tool_name(), model_spec)?;
            cfg.enforce_tier_model_name(executor.tool_name(), model)?;
        }

        // Enforce thinking level is configured in tiers (unless force override).
        // Use the effective thinking level (after thinking_lock override), not the
        // original CLI value, to avoid rejecting locked values that differ from CLI.
        let effective_thinking = lock_from_project.or(lock_from_global).or(thinking_budget);
        if enforce_tier && !force_override_user_config {
            cfg.enforce_thinking_level(effective_thinking)?;
        }
    }

    // Check tool is installed
    if let Err(e) = check_tool_installed(executor.runtime_binary_name()).await {
        error!(
            "Tool '{}' is not installed.\n\n{}\n\nOr disable it in .csa/config.toml:\n  [tools.{}]\n  enabled = false",
            executor.tool_name(),
            executor.install_hint(),
            executor.tool_name()
        );
        anyhow::bail!("{}", e);
    }
    ensure_tool_runtime_prerequisites(executor.tool_name()).await?;

    Ok(executor)
}

async fn ensure_tool_runtime_prerequisites(tool_name: &str) -> Result<()> {
    if tool_name != "codex" {
        return Ok(());
    }
    if std::env::var("CSA_SKIP_BWRAP_PREFLIGHT").ok().as_deref() == Some("1") {
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        let has_bwrap = tokio::process::Command::new("which")
            .arg("bwrap")
            .output()
            .await
            .map(|out| out.status.success())
            .unwrap_or(false);
        if !has_bwrap {
            anyhow::bail!(
                "codex preflight failed: required runtime dependency 'bwrap' (bubblewrap) is missing.\n\
                 Install bubblewrap first, then re-run the command."
            );
        }
    }

    Ok(())
}

/// Acquire global concurrency slot for the executor.
///
/// Returns ToolSlot guard on success.
/// Returns error if all slots occupied (no failover here).
#[tracing::instrument(skip_all, fields(tool = %executor.tool_name()))]
pub(crate) fn acquire_slot(
    executor: &Executor,
    global_config: &GlobalConfig,
) -> Result<csa_lock::slot::ToolSlot> {
    let max_concurrent = global_config.max_concurrent(executor.tool_name());
    let slots_dir = GlobalConfig::slots_dir()?;

    match csa_lock::slot::try_acquire_slot(&slots_dir, executor.tool_name(), max_concurrent, None) {
        Ok(csa_lock::slot::SlotAcquireResult::Acquired(slot)) => Ok(slot),
        Ok(csa_lock::slot::SlotAcquireResult::Exhausted(status)) => {
            anyhow::bail!(
                "All {} slots for '{}' occupied ({}/{}). Try again later or use --tool to switch.",
                max_concurrent,
                executor.tool_name(),
                status.occupied,
                status.max_slots,
            )
        }
        Err(e) => anyhow::bail!(
            "Slot acquisition failed for '{}': {}",
            executor.tool_name(),
            e
        ),
    }
}

/// Execution result with the resolved CSA meta session ID used by this run.
pub(crate) struct SessionExecutionResult {
    pub execution: ExecutionResult,
    pub meta_session_id: String,
    pub provider_session_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MemoryInjectionOptions {
    pub disabled: bool,
    pub query_override: Option<String>,
}

pub(crate) fn run_pipeline_hook(
    event: HookEvent,
    hooks_config: &csa_hooks::HooksConfig,
    variables: &std::collections::HashMap<String, String>,
) -> Result<()> {
    run_hooks_for_event(event, hooks_config, variables).map_err(|err| {
        anyhow::anyhow!("{event:?} hook failed and fail_policy=closed blocked execution: {err}")
    })
}

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
    memory_injection: Option<&MemoryInjectionOptions>,
    global_config: Option<&GlobalConfig>,
) -> Result<ExecutionResult> {
    let execution = execute_with_session_and_meta(
        executor,
        tool,
        prompt,
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
    memory_injection: Option<&MemoryInjectionOptions>,
    global_config: Option<&GlobalConfig>,
) -> Result<SessionExecutionResult> {
    // Check for parent session violation: a child process must not operate on its own session
    if let Some(ref session_id) = session_arg {
        if let Ok(env_session) = std::env::var("CSA_SESSION_ID") {
            if env_session == *session_id {
                return Err(csa_core::error::AppError::ParentSessionViolation.into());
            }
        }
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
        let parent_id = parent.or_else(|| std::env::var("CSA_SESSION_ID").ok());
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
        if let (Some(cfg), Some(tier)) = (config, tier_name) {
            if let Some(tier_cfg) = cfg.tiers.get(tier) {
                if tier_cfg.token_budget.is_some() || tier_cfg.max_turns.is_some() {
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
            }
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
    let (_log_writer, _log_guard) = match create_session_log_writer(&session_dir) {
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
    if let Some(ref mut guard) = resource_guard {
        if let Err(e) = guard.check_availability(executor.tool_name()) {
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
    if !can_edit {
        info!(tool = %executor.tool_name(), "Applying edit restriction: tool cannot modify existing files");
        effective_prompt = executor.apply_restrictions(&effective_prompt, false);
    }

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
        SessionConfig {
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

    // Load hooks config once, reused by PreRun, PostRun, and SessionComplete hooks.
    let hooks_config = load_hooks_config(
        csa_session::get_session_root(project_root)
            .ok()
            .map(|r| r.join("hooks.toml"))
            .as_deref(),
        global_hooks_path().as_deref(),
        None,
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
    ) {
        crate::pipeline_sandbox::SandboxResolution::Ok(opts) => opts,
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
    let events_count = transport_result.events.len() as u64;
    let transcript_artifacts =
        crate::pipeline_transcript::persist_if_enabled(config, &session_dir, &transport_result);
    let result = transport_result.execution;

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
    crate::pipeline_post_exec::process_execution_result(post_ctx, &mut session, &result).await?;

    Ok(SessionExecutionResult {
        execution: result,
        meta_session_id: session.meta_session_id.clone(),
        provider_session_id,
    })
}

pub(crate) fn determine_project_root(cd: Option<&str>) -> Result<PathBuf> {
    let path = if let Some(cd_path) = cd {
        PathBuf::from(cd_path)
    } else {
        std::env::current_dir()?
    };

    Ok(path.canonicalize()?)
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pipeline_tests_thinking.rs"]
mod thinking_tests;

#[cfg(test)]
#[path = "pipeline_tests_prompt_guard.rs"]
mod prompt_guard_tests;
