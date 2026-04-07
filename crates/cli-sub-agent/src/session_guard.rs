//! RAII guard for session cleanup and pre-execution error recording.

use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use csa_session::{SessionResult, create_session, save_result, save_session};

/// RAII guard that cleans up a newly created session directory on failure.
///
/// When `execute_with_session` creates a new session but the tool fails to spawn
/// (or any pre-execution step errors out), the session directory would remain on
/// disk as an orphan. This guard deletes it automatically on drop unless
/// `defuse()` is called after successful tool execution. Once the tool has
/// produced output, the session directory is preserved even if later persistence
/// steps (save_session, hooks) fail.
///
/// ## Cleanup flow in `execute_with_session_and_meta`
///
/// The guard is armed for new sessions only (not resumed ones). The following
/// pre-execution failure paths each write a `result.toml` via
/// [`write_pre_exec_error_result`] then **defuse** the guard so the session
/// directory is preserved with a failure record:
///
/// 1. `create_session_log_writer` fails
/// 2. `acquire_lock` fails
/// 3. `ResourceGuard::check_availability` fails
/// 4. `executor.execute_with_transport` fails (spawn error)
///
/// If none of these fail, the guard is defused after successful tool execution
/// (line after `execute_with_transport` returns `Ok`). Later failures (e.g.,
/// `save_session`, hooks) do NOT re-arm the guard — the session directory is
/// preserved because it contains execution output worth keeping.
///
/// If the guard is **not** defused (e.g., a future pre-execution step is added
/// without a corresponding `write_pre_exec_error_result` + `defuse()`), the
/// `Drop` impl will remove the orphan directory entirely.
pub(crate) struct SessionCleanupGuard {
    session_dir: PathBuf,
    defused: bool,
}

impl SessionCleanupGuard {
    pub(crate) fn new(session_dir: PathBuf) -> Self {
        Self {
            session_dir,
            defused: false,
        }
    }

    pub(crate) fn defuse(&mut self) {
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

/// Write an error result.toml for pre-execution failures.
///
/// Called when the session directory exists but the tool never executed
/// (e.g., spawn failure, resource exhaustion). Preserves the session directory
/// so downstream tools can see the failure instead of an orphan with no result.
pub(crate) fn write_pre_exec_error_result(
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
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
    };
    if let Err(e) = save_result(project_root, session_id, &result) {
        warn!("Failed to save pre-execution error result: {}", e);
    }
    // Best-effort cooldown marker
    csa_session::write_cooldown_marker_for_project(project_root, session_id, now);
}

fn create_pre_exec_failure_session(
    project_root: &Path,
    description: Option<&str>,
    parent: Option<&str>,
    tool_name: Option<&str>,
    task_type: Option<&str>,
    tier_name: Option<&str>,
) -> anyhow::Result<String> {
    let mut session = create_session(project_root, description, parent, tool_name)?;
    session.task_context.task_type = task_type.map(str::to_string);
    session.task_context.tier_name = tier_name.map(str::to_string);
    let session_id = session.meta_session_id.clone();
    if let Err(err) = save_session(&session) {
        warn!(
            session_id = %session_id,
            error = %err,
            "Failed to persist task context for pre-exec failure session"
        );
    }
    Ok(session_id)
}

pub(crate) struct PreExecErrorCtx<'ctx> {
    pub(crate) project_root: &'ctx Path,
    pub(crate) session_id: Option<&'ctx str>,
    pub(crate) description: Option<&'ctx str>,
    pub(crate) parent: Option<&'ctx str>,
    pub(crate) tool_name: Option<&'ctx str>,
    pub(crate) task_type: Option<&'ctx str>,
    pub(crate) tier_name: Option<&'ctx str>,
    pub(crate) error: anyhow::Error,
}

/// Persist a structured pre-exec failure result, creating a session when needed.
///
/// Returns the original error annotated with `meta_session_id=...` when a session
/// is available so downstream error handlers can still recover the session ID.
pub(crate) fn persist_pre_exec_error_result(ctx: PreExecErrorCtx<'_>) -> anyhow::Error {
    let PreExecErrorCtx {
        project_root,
        session_id,
        description,
        parent,
        tool_name,
        task_type,
        tier_name,
        error,
    } = ctx;
    let recorded_session_id = match session_id {
        Some(existing) => Some(existing.to_string()),
        None => match create_pre_exec_failure_session(
            project_root,
            description,
            parent,
            tool_name,
            task_type,
            tier_name,
        ) {
            Ok(created) => Some(created),
            Err(create_err) => {
                warn!(
                    error = %create_err,
                    task_type = task_type.unwrap_or("unknown"),
                    "Failed to create session for pre-exec error result"
                );
                None
            }
        },
    };

    if let Some(ref sid) = recorded_session_id {
        write_pre_exec_error_result(project_root, sid, tool_name.unwrap_or("unknown"), &error);
        error.context(format!("meta_session_id={sid}"))
    } else {
        error
    }
}
