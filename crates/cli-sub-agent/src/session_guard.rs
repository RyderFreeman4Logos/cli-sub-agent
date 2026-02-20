//! RAII guard for session cleanup and pre-execution error recording.

use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use csa_session::{SessionResult, save_result};

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
    };
    if let Err(e) = save_result(project_root, session_id, &result) {
        warn!("Failed to save pre-execution error result: {}", e);
    }
}
