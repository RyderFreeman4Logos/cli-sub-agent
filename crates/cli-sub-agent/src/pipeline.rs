//! Shared execution pipeline functions for CSA command handlers.
//!
//! This module extracts common patterns from run, review, and debate handlers:
//! - Config loading and recursion depth validation
//! - Executor building and tool installation checks
//! - Global slot acquisition with concurrency limits

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{error, info, warn};

use csa_config::{GlobalConfig, McpRegistry, ProjectConfig};
use csa_core::types::ToolName;
use csa_executor::{AcpMcpServerConfig, Executor, SessionConfig, create_session_log_writer};
use csa_hooks::{HookEvent, global_hooks_path, load_hooks_config, run_hooks_for_event};
use csa_lock::acquire_lock;
use csa_process::{ExecutionResult, check_tool_installed};
use csa_resource::{ResourceGuard, ResourceLimits};
use csa_session::{
    SessionResult, TokenUsage, ToolState, create_session, get_session_dir, save_result,
    save_session,
};

use crate::run_helpers::{is_compress_command, parse_token_usage, truncate_prompt};

/// RAII guard that cleans up a newly created session directory on failure.
///
/// When `execute_with_session` creates a new session but the tool fails to spawn
/// (or any pre-execution step errors out), the session directory would remain on
/// disk as an orphan. This guard deletes it automatically on drop unless
/// `defuse()` is called after successful tool execution. Once the tool has
/// produced output, the session directory is preserved even if later persistence
/// steps (save_session, hooks) fail.
struct SessionCleanupGuard {
    session_dir: PathBuf,
    defused: bool,
}

impl SessionCleanupGuard {
    fn new(session_dir: PathBuf) -> Self {
        Self {
            session_dir,
            defused: false,
        }
    }

    fn defuse(&mut self) {
        self.defused = true;
    }
}

impl Drop for SessionCleanupGuard {
    fn drop(&mut self) {
        if !self.defused {
            info!(
                dir = %self.session_dir.display(),
                "Cleaning up orphan session directory"
            );
            if let Err(e) = fs::remove_dir_all(&self.session_dir) {
                warn!("Failed to clean up orphan session: {}", e);
            }
        }
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
            return global_servers.iter().map(config_to_acp_mcp).collect();
        }
        Err(e) => {
            warn!("Failed to load project MCP registry: {e}");
            return global_servers.iter().map(config_to_acp_mcp).collect();
        }
    };

    let merged = McpRegistry::merge(global_servers, &project_registry);
    merged.servers.iter().map(config_to_acp_mcp).collect()
}

/// Convert `csa_config::McpServerConfig` to [`AcpMcpServerConfig`].
///
/// The two types have identical fields but live in separate crates to avoid
/// coupling `csa-acp` (protocol layer) to `csa-config` (user config layer).
fn config_to_acp_mcp(cfg: &csa_config::McpServerConfig) -> AcpMcpServerConfig {
    AcpMcpServerConfig {
        name: cfg.name.clone(),
        command: cfg.command.clone(),
        args: cfg.args.clone(),
        env: cfg.env.clone(),
    }
}

/// Build executor and validate tool is installed and enabled.
///
/// Returns Executor on success.
/// Returns error if tool not installed or disabled in config.
pub(crate) async fn build_and_validate_executor(
    tool: &ToolName,
    model_spec: Option<&str>,
    model: Option<&str>,
    thinking_budget: Option<&str>,
    config: Option<&ProjectConfig>,
) -> Result<Executor> {
    let executor =
        crate::run_helpers::build_executor(tool, model_spec, model, thinking_budget, config)?;

    // Check tool is enabled in config (before checking installation)
    if let Some(cfg) = config {
        if !cfg.is_tool_enabled(executor.tool_name()) {
            error!(
                "Tool '{}' is disabled in project config",
                executor.tool_name()
            );
            anyhow::bail!("Tool disabled in config");
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

    Ok(executor)
}

/// Acquire global concurrency slot for the executor.
///
/// Returns ToolSlot guard on success.
/// Returns error if all slots occupied (no failover here).
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

/// Write an error result.toml for pre-execution failures.
///
/// Called when the session directory exists but the tool never executed
/// (e.g., spawn failure, resource exhaustion). Preserves the session directory
/// so downstream tools can see the failure instead of an orphan with no result.
fn write_pre_exec_error_result(
    project_root: &Path,
    session_id: &str,
    tool_name: &str,
    error: &anyhow::Error,
) {
    let now = chrono::Utc::now();
    let result = SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary: format!("pre-exec: {error}"),
        tool: tool_name.to_string(),
        started_at: now,
        completed_at: now,
        artifacts: Vec::new(),
    };
    if let Err(e) = save_result(project_root, session_id, &result) {
        warn!("Failed to save pre-execution error result: {}", e);
    }
}

/// Execution result with the resolved CSA meta session ID used by this run.
pub(crate) struct SessionExecutionResult {
    pub execution: ExecutionResult,
    pub meta_session_id: String,
    pub provider_session_id: Option<String>,
}

#[allow(clippy::too_many_arguments)]
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
    stream_mode: csa_process::StreamMode,
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
        stream_mode,
        global_config,
    )
    .await?;

    Ok(execution.execution)
}

#[allow(clippy::too_many_arguments)]
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
    stream_mode: csa_process::StreamMode,
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

    let session_dir = get_session_dir(project_root, &session.meta_session_id)?;

    // Arm cleanup guard for new sessions only (not resumed ones).
    // If any pre-execution step fails, the guard deletes the orphan directory.
    let mut cleanup_guard = if session_arg.is_none() {
        Some(SessionCleanupGuard::new(session_dir.clone()))
    } else {
        None
    };

    // Create session log writer
    let (_log_writer, _log_guard) =
        create_session_log_writer(&session_dir).context("Failed to create session log writer")?;

    // Acquire lock with truncated prompt as reason
    let lock_reason = truncate_prompt(prompt, 80);
    let _lock =
        acquire_lock(&session_dir, executor.tool_name(), &lock_reason).with_context(|| {
            format!(
                "Failed to acquire lock for session {}",
                session.meta_session_id
            )
        })?;

    // Resource guard
    let mut resource_guard = if let Some(cfg) = config {
        let limits = ResourceLimits {
            min_free_memory_mb: cfg.resources.min_free_memory_mb,
            initial_estimates: cfg.resources.initial_estimates.clone(),
        };
        // Stats stored at project state level, not per-session
        let project_state_dir = csa_session::get_session_root(project_root)?;
        let stats_path = project_state_dir.join("usage_stats.toml");
        Some(ResourceGuard::new(limits, &stats_path))
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

    // Check token budget before execution
    if let Some(ref budget) = session.token_budget {
        if budget.is_hard_exceeded() {
            let err = anyhow::anyhow!(
                "Token budget exhausted: used {} / {} allocated ({}%)",
                budget.used,
                budget.allocated,
                budget.usage_pct()
            );
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
        if budget.is_turns_exceeded(session.turn_count) {
            let err = anyhow::anyhow!(
                "Max turns exceeded: {} / {} allowed",
                session.turn_count,
                budget.max_turns.unwrap_or(0)
            );
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

    // Apply restrictions if configured
    let can_edit = config.is_none_or(|cfg| cfg.can_tool_edit_existing(executor.tool_name()));
    let mut effective_prompt = if !can_edit {
        info!(tool = %executor.tool_name(), "Applying edit restriction: tool cannot modify existing files");
        executor.apply_restrictions(prompt, false)
    } else {
        prompt.to_string()
    };

    // Auto-inject project context (CLAUDE.md, AGENTS.md) on first turn only.
    // Session resumes already have context loaded in the tool's conversation.
    let is_first_turn = session
        .tools
        .get(executor.tool_name())
        .is_none_or(|ts| ts.provider_session_id.is_none());
    if is_first_turn {
        let context_files = csa_executor::load_project_context(
            Path::new(&session.project_path),
            &csa_executor::ContextLoadOptions::default(),
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
            ..Default::default()
        }
    });

    // Execute via transport abstraction.
    // TODO(signal): Restore SIGINT/SIGTERM forwarding to child process groups.
    // Phase C moved signal handling responsibility to the Transport layer, but
    // AcpTransport does not yet propagate signals. LegacyTransport inherits
    // csa-process::wait_and_capture which handles signals via process groups.
    let transport_result = match executor
        .execute_with_transport(
            &effective_prompt,
            tool_state.as_ref(),
            &session,
            extra_env,
            stream_mode,
            session_config,
        )
        .await
    {
        Ok(result) => result,
        Err(e) => {
            write_pre_exec_error_result(
                project_root,
                &session.meta_session_id,
                executor.tool_name(),
                &e,
            );
            if let Some(ref mut cg) = cleanup_guard {
                cg.defuse();
            }
            return Err(e).context("Failed to execute tool via transport");
        }
    };

    // Tool execution completed — defuse cleanup guard now.
    // The session directory contains execution artifacts worth preserving
    // even if a later persistence step (save_session, hooks) fails.
    if let Some(ref mut guard) = cleanup_guard {
        guard.defuse();
    }

    // Extract provider session ID from transport metadata or fallback output parsing.
    let provider_session_id =
        csa_executor::extract_session_id_from_transport(tool, &transport_result);
    let result = transport_result.execution;

    // Parse token usage from output (best-effort)
    let token_usage = parse_token_usage(&result.output);

    // Update session state
    session
        .tools
        .entry(executor.tool_name().to_string())
        .and_modify(|t| {
            // Only update provider_session_id if extraction succeeded
            if let Some(ref session_id) = provider_session_id {
                t.provider_session_id = Some(session_id.clone());
            }
            t.last_action_summary = result.summary.clone();
            t.last_exit_code = result.exit_code;
            t.updated_at = chrono::Utc::now();

            // Update token usage if parsed successfully
            if let Some(ref usage) = token_usage {
                t.token_usage = Some(usage.clone());
            }
        })
        .or_insert_with(|| ToolState {
            provider_session_id: provider_session_id.clone(),
            last_action_summary: result.summary.clone(),
            last_exit_code: result.exit_code,
            updated_at: chrono::Utc::now(),
            token_usage: token_usage.clone(),
        });
    session.last_accessed = chrono::Utc::now();

    // Detect compress/compact commands: mark session as Available for reuse
    if result.exit_code == 0 && is_compress_command(prompt) {
        session.context_status.is_compacted = true;
        session.context_status.last_compacted_at = Some(chrono::Utc::now());
        match session
            .phase
            .transition(&csa_session::PhaseEvent::Compressed)
        {
            Ok(new_phase) => {
                session.phase = new_phase;
                info!(
                    session = %session.meta_session_id,
                    "Session compacted and marked Available for reuse"
                );
            }
            Err(e) => {
                warn!(
                    session = %session.meta_session_id,
                    error = %e,
                    "Skipping phase transition on compress"
                );
            }
        }
    }

    // Increment turn count
    session.turn_count += 1;

    // Update cumulative token usage if we got new tokens
    if let Some(new_usage) = token_usage {
        let cumulative = session
            .total_token_usage
            .get_or_insert(TokenUsage::default());
        cumulative.input_tokens =
            Some(cumulative.input_tokens.unwrap_or(0) + new_usage.input_tokens.unwrap_or(0));
        cumulative.output_tokens =
            Some(cumulative.output_tokens.unwrap_or(0) + new_usage.output_tokens.unwrap_or(0));
        cumulative.total_tokens =
            Some(cumulative.total_tokens.unwrap_or(0) + new_usage.total_tokens.unwrap_or(0));
        cumulative.estimated_cost_usd = Some(
            cumulative.estimated_cost_usd.unwrap_or(0.0)
                + new_usage.estimated_cost_usd.unwrap_or(0.0),
        );

        // Update token budget tracking
        if let Some(ref mut budget) = session.token_budget {
            let tokens_used = new_usage.total_tokens.unwrap_or(0);
            budget.record_usage(tokens_used);
            if budget.is_hard_exceeded() {
                warn!(
                    session = %session.meta_session_id,
                    used = budget.used,
                    allocated = budget.allocated,
                    "Token budget hard threshold reached — next execution will be blocked"
                );
            } else if budget.is_soft_exceeded() {
                warn!(
                    session = %session.meta_session_id,
                    used = budget.used,
                    allocated = budget.allocated,
                    remaining = budget.remaining(),
                    "Token budget soft threshold reached"
                );
            }
        }
    }

    // Write prompt to input/ for audit trail
    let input_dir = session_dir.join("input");
    if input_dir.exists() {
        let prompt_path = input_dir.join("prompt.txt");
        if let Err(e) = fs::write(&prompt_path, prompt) {
            warn!("Failed to write prompt to input/: {}", e);
        }
    }

    // Write structured result
    let execution_end_time = chrono::Utc::now();
    let session_result = SessionResult {
        status: SessionResult::status_from_exit_code(result.exit_code),
        exit_code: result.exit_code,
        summary: result.summary.clone(),
        tool: executor.tool_name().to_string(),
        started_at: execution_start_time,
        completed_at: execution_end_time,
        artifacts: Vec::new(), // populated by hooks later (Phase 3.3)
    };
    if let Err(e) = save_result(project_root, &session.meta_session_id, &session_result) {
        warn!("Failed to save session result: {}", e);
    }

    // Save session
    save_session(&session)?;

    // Fire PostRun and SessionComplete hooks (best-effort)
    let project_hooks_path = csa_session::get_session_root(project_root)
        .ok()
        .map(|root| root.join("hooks.toml"));
    let hooks_config = load_hooks_config(
        project_hooks_path.as_deref(),
        global_hooks_path().as_deref(),
        None,
    );
    let mut hook_vars = std::collections::HashMap::new();
    hook_vars.insert("session_id".to_string(), session.meta_session_id.clone());
    hook_vars.insert("session_dir".to_string(), session_dir.display().to_string());
    hook_vars.insert(
        "sessions_root".to_string(),
        session_dir
            .parent()
            .unwrap_or(&session_dir)
            .display()
            .to_string(),
    );
    hook_vars.insert("tool".to_string(), executor.tool_name().to_string());
    hook_vars.insert("exit_code".to_string(), result.exit_code.to_string());

    // PostRun hook: fires after every tool execution
    if let Err(e) = run_hooks_for_event(HookEvent::PostRun, &hooks_config, &hook_vars) {
        warn!("PostRun hook failed: {}", e);
    }

    // SessionComplete hook: git-commits session artifacts
    if let Err(e) = run_hooks_for_event(HookEvent::SessionComplete, &hooks_config, &hook_vars) {
        warn!("SessionComplete hook failed: {}", e);
    }

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
mod tests {
    use super::*;

    #[test]
    fn determine_project_root_none_returns_cwd() {
        let result = determine_project_root(None).unwrap();
        let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
        assert_eq!(result, cwd);
    }

    #[test]
    fn determine_project_root_with_valid_path() {
        let tmp = tempfile::tempdir().unwrap();
        let result = determine_project_root(Some(tmp.path().to_str().unwrap())).unwrap();
        assert_eq!(result, tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn determine_project_root_nonexistent_path_errors() {
        let result = determine_project_root(Some("/nonexistent/path/12345"));
        assert!(result.is_err());
    }

    #[test]
    fn load_and_validate_exceeds_depth_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        // With no config, max_depth defaults to 5
        let result = load_and_validate(tmp.path(), 100).unwrap();
        assert!(
            result.is_none(),
            "Should return None when depth exceeds max"
        );
    }

    #[test]
    fn load_and_validate_within_depth_returns_some() {
        let tmp = tempfile::tempdir().unwrap();
        let result = load_and_validate(tmp.path(), 0).unwrap();
        assert!(
            result.is_some(),
            "Should return Some when depth is within bounds"
        );
    }
}
