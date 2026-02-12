//! Shared execution pipeline functions for CSA command handlers.
//!
//! This module extracts common patterns from run, review, and debate handlers:
//! - Config loading and recursion depth validation
//! - Executor building and tool installation checks
//! - Global slot acquisition with concurrency limits

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, warn};

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_executor::{create_session_log_writer, Executor};
use csa_hooks::{global_hooks_path, load_hooks_config, run_hooks_for_event, HookEvent};
use csa_lock::acquire_lock;
use csa_process::check_tool_installed;
use csa_resource::{MemoryMonitor, ResourceGuard, ResourceLimits};
use csa_session::{
    create_session, get_session_dir, load_session, resolve_session_prefix, save_result,
    save_session, SessionResult, TokenUsage, ToolState,
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
    if let Err(e) = check_tool_installed(executor.executable_name()).await {
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
) -> Result<csa_process::ExecutionResult> {
    // Check for parent session violation: a child process must not operate on its own session
    if let Some(ref session_id) = session_arg {
        if let Ok(env_session) = std::env::var("CSA_SESSION_ID") {
            if env_session == *session_id {
                return Err(csa_core::error::AppError::ParentSessionViolation.into());
            }
        }
    }

    // Resolve or create session
    let mut session = if let Some(ref session_id) = session_arg {
        let sessions_dir = csa_session::get_session_root(project_root)?.join("sessions");
        let resolved_id = resolve_session_prefix(&sessions_dir, session_id)?;
        // Validate tool access before loading
        csa_session::validate_tool_access(project_root, &resolved_id, tool.as_str())?;
        load_session(project_root, &resolved_id)?
    } else {
        let parent_id = parent.or_else(|| std::env::var("CSA_SESSION_ID").ok());
        create_session(
            project_root,
            description.as_deref(),
            parent_id.as_deref(),
            Some(tool.as_str()),
        )?
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
        guard.check_availability(executor.tool_name())?;
    }

    info!("Executing in session: {}", session.meta_session_id);

    // Apply restrictions if configured
    let can_edit = config.map_or(true, |cfg| cfg.can_tool_edit_existing(executor.tool_name()));
    let effective_prompt = if !can_edit {
        info!(tool = %executor.tool_name(), "Applying edit restriction: tool cannot modify existing files");
        executor.apply_restrictions(prompt, false)
    } else {
        prompt.to_string()
    };

    // Build command
    let tool_state = session.tools.get(executor.tool_name()).cloned();
    let cmd = executor.build_command(&effective_prompt, tool_state.as_ref(), &session, extra_env);

    // Record execution start time before spawning
    let execution_start_time = chrono::Utc::now();

    // Spawn child process
    let child = csa_process::spawn_tool(cmd)
        .await
        .context("Failed to spawn tool process")?;

    // Get child PID and start memory monitor
    let child_pid = child.id().context("Failed to get child process PID")?;
    let monitor = MemoryMonitor::start(child_pid);

    // Set up signal handlers for SIGTERM and SIGINT
    let mut sigterm =
        signal(SignalKind::terminate()).context("Failed to install SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("Failed to install SIGINT handler")?;

    // Wait for either child completion or signal
    let wait_future = csa_process::wait_and_capture(child);
    tokio::pin!(wait_future);

    let result = tokio::select! {
        result = &mut wait_future => {
            result.context("Failed to wait for tool process")?
        }
        _ = sigterm.recv() => {
            info!("Received SIGTERM, forwarding to child process group");
            // Forward SIGTERM to the child's process group (negative PID)
            // SAFETY: kill() is async-signal-safe. We use the negative of child_pid
            // to target the entire process group created by setsid().
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child_pid as i32), libc::SIGTERM);
            }
            // Wait for child to exit after signal
            wait_future.await.context("Failed to wait for tool process after SIGTERM")?
        }
        _ = sigint.recv() => {
            info!("Received SIGINT, forwarding to child process group");
            // Forward SIGINT to the child's process group
            // SAFETY: Same as SIGTERM handler above
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child_pid as i32), libc::SIGINT);
            }
            // Wait for child to exit after signal
            wait_future.await.context("Failed to wait for tool process after SIGINT")?
        }
    };

    // Stop memory monitor and record usage
    let peak_memory_mb = monitor.stop().await;
    if let Some(ref mut guard) = resource_guard {
        guard.record_usage(executor.tool_name(), peak_memory_mb);
    }

    // Tool execution completed — defuse cleanup guard now.
    // The session directory contains execution artifacts worth preserving
    // even if a later persistence step (save_session, hooks) fails.
    if let Some(ref mut guard) = cleanup_guard {
        guard.defuse();
    }

    // Extract provider session ID from output
    let provider_session_id = csa_executor::extract_session_id(tool, &result.output);

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
            provider_session_id,
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
        session.phase = csa_session::SessionPhase::Available;
        info!(
            session = %session.meta_session_id,
            "Session compacted and marked Available for reuse"
        );
    }

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

    Ok(result)
}

pub(crate) fn determine_project_root(cd: Option<&str>) -> Result<PathBuf> {
    let path = if let Some(cd_path) = cd {
        PathBuf::from(cd_path)
    } else {
        std::env::current_dir()?
    };

    Ok(path.canonicalize()?)
}
