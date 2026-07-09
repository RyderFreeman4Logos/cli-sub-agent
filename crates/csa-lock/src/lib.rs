//! File-based locking using `flock(2)` syscall directly.
//! Independent crate with no internal csa dependencies.
//!
//! Uses raw `libc::flock` instead of RAII lock wrappers to avoid the
//! self-referential struct problem: an RAII guard borrows the lock owner,
//! making it impossible to store both in the same struct without lifetime
//! gymnastics (`Box::leak`, `ouroboros`, etc.).
//!
//! By calling `flock(2)` directly, we only need to own the `File` (which
//! owns the fd). `Drop` calls `flock(fd, LOCK_UN)` to release.

pub mod slot;
mod worktree;

pub use worktree::{
    WorktreeWriteLock, acquire_worktree_write_lock, worktree_write_lock_is_held_by_session,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};

/// Diagnostic information written to lock files
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LockDiagnostic {
    pub(crate) pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pid_start_time_ticks: Option<u64>,
    tool_name: String,
    pub(crate) acquired_at: DateTime<Utc>,
    reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) holder_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resource_path: Option<String>,
}

/// Session lock guard backed by `flock(2)`.
///
/// Holds the open `File` whose fd carries the advisory lock.
/// On `Drop`, the lock is explicitly released via `flock(fd, LOCK_UN)`.
pub struct SessionLock {
    /// The open lock file. Closing it also releases flock, but we call
    /// `LOCK_UN` explicitly in `Drop` for deterministic release timing.
    file: File,
    lock_path: PathBuf,
}

impl std::fmt::Debug for SessionLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionLock")
            .field("lock_path", &self.lock_path)
            .finish()
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        let fd = self.file.as_raw_fd();
        // SAFETY: `fd` is a valid file descriptor owned by `self.file`.
        // `LOCK_UN` releases the advisory lock. If the call fails (which is
        // extremely unlikely for a valid fd), the lock will still be released
        // when the fd is closed moments later.
        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
    }
}

impl SessionLock {
    /// Get the path to the lock file
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}

fn sanitize_lock_component(input: &str) -> String {
    let sanitized: String = input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn acquire_lock_at_path(
    lock_path: &Path,
    lock_name: &str,
    reason: &str,
) -> Result<SessionLock> {
    acquire_lock_at_path_with_metadata(lock_path, lock_name, reason, None, None)
}

pub(crate) fn acquire_lock_at_path_with_metadata(
    lock_path: &Path,
    lock_name: &str,
    reason: &str,
    holder_session_id: Option<&str>,
    resource_path: Option<&Path>,
) -> Result<SessionLock> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create locks directory: {}", parent.display()))?;
    }

    if let Some(lock) = try_acquire_lock_at_path(
        lock_path,
        lock_name,
        reason,
        holder_session_id,
        resource_path,
    )? {
        return Ok(lock);
    }

    // Lock is held by another process, try to read diagnostic info.
    let diagnostic = read_lock_diagnostic(lock_path)?;
    let error_msg = diagnostic
        .as_ref()
        .map(|diagnostic| format_lock_diagnostic(lock_path, diagnostic))
        .unwrap_or_else(|| "Session is locked (unable to read diagnostic info)".to_string());

    // Resource-scoped locks such as the worktree write lock layer their own
    // stale recovery on top of this primitive. The generic session-lock
    // force-clear path must not bypass those resource-specific safety checks.
    if resource_path.is_none()
        && let Some(diagnostic) = diagnostic.as_ref()
        && is_pid_dead(diagnostic.pid, diagnostic.pid_start_time_ticks)
    {
        warn_stale_lock_recovery(lock_path, diagnostic);
        if clear_stale_lock_file(lock_path) {
            match try_acquire_lock_at_path(
                lock_path,
                lock_name,
                reason,
                holder_session_id,
                resource_path,
            ) {
                Ok(Some(lock)) => return Ok(lock),
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    lock_path = %lock_path.display(),
                    error = %error,
                    "failed to retry stale session lock acquisition"
                ),
            }
        }
    }

    Err(anyhow::anyhow!(error_msg))
}

fn try_acquire_lock_at_path(
    lock_path: &Path,
    lock_name: &str,
    reason: &str,
    holder_session_id: Option<&str>,
    resource_path: Option<&Path>,
) -> Result<Option<SessionLock>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .with_context(|| format!("Failed to open lock file: {}", lock_path.display()))?;

    let fd = file.as_raw_fd();

    // SAFETY: `fd` is a valid file descriptor from the `File` we just opened.
    // `LOCK_EX | LOCK_NB` requests an exclusive non-blocking lock.
    // The return value is checked for error handling.
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

    if ret == 0 {
        set_fd_cloexec(fd, lock_path)?;

        // Lock acquired successfully. Write diagnostic information.
        let mut lock = SessionLock {
            file,
            lock_path: lock_path.to_path_buf(),
        };

        let diagnostic = LockDiagnostic {
            pid: std::process::id(),
            pid_start_time_ticks: process_start_time_ticks(std::process::id()),
            tool_name: lock_name.to_string(),
            acquired_at: Utc::now(),
            reason: reason.to_string(),
            holder_session_id: holder_session_id.map(ToString::to_string),
            resource_path: resource_path.map(|path| path.display().to_string()),
        };

        let json =
            serde_json::to_string(&diagnostic).context("Failed to serialize lock diagnostic")?;

        lock.file
            .set_len(0)
            .context("Failed to truncate lock file")?;
        lock.file
            .write_all(json.as_bytes())
            .context("Failed to write lock diagnostic")?;
        lock.file.flush().context("Failed to flush lock file")?;

        Ok(Some(lock))
    } else {
        Ok(None)
    }
}

pub(crate) fn set_fd_cloexec(fd: RawFd, path: &Path) -> Result<()> {
    // SAFETY: `fd` is owned by a live `File`; F_GETFD reads descriptor flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("Failed to read fd flags for {}", path.display()));
    }

    // SAFETY: `fd` is valid, and F_SETFD updates only close-on-exec flags.
    let ret = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    if ret == -1 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("Failed to set FD_CLOEXEC on {}", path.display()));
    }

    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn process_start_time_ticks(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(") ")?.1;
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn process_start_time_ticks(_pid: u32) -> Option<u64> {
    None
}

/// Check whether a PID is dead or has been recycled since it acquired the lock.
pub(crate) fn is_pid_dead(pid: u32, pid_start_time_ticks: Option<u64>) -> bool {
    let Ok(pid_i32) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: `kill(pid, 0)` sends no signal; it only checks whether the
    // process currently exists and is visible to this process.
    let alive = unsafe { libc::kill(pid_i32, 0) } == 0;
    if !alive {
        let error = std::io::Error::last_os_error();
        return error.raw_os_error() == Some(libc::ESRCH);
    }

    if let Some(expected_ticks) = pid_start_time_ticks
        && let Some(current_ticks) = process_start_time_ticks(pid)
    {
        return current_ticks != expected_ticks;
    }

    false
}

fn warn_stale_lock_recovery(lock_path: &Path, diagnostic: &LockDiagnostic) {
    let held_for_seconds = Utc::now()
        .signed_duration_since(diagnostic.acquired_at)
        .num_seconds()
        .max(0);
    tracing::warn!(
        lock_path = %lock_path.display(),
        holder_pid = diagnostic.pid,
        holder_session = ?diagnostic.holder_session_id,
        lock_held_seconds = held_for_seconds,
        "clearing stale session lock (holder PID is dead or recycled)"
    );
}

fn clear_stale_lock_file(lock_path: &Path) -> bool {
    match fs::remove_file(lock_path) {
        Ok(()) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => true,
        Err(error) => {
            tracing::warn!(
                lock_path = %lock_path.display(),
                error = %error,
                "failed to clear stale session lock file"
            );
            false
        }
    }
}

fn format_lock_diagnostic(lock_path: &Path, diagnostic: &LockDiagnostic) -> String {
    let holder = diagnostic
        .holder_session_id
        .as_deref()
        .map(|session| format!(", holder_session_id: {session}"))
        .unwrap_or_default();
    let resource = diagnostic
        .resource_path
        .as_deref()
        .map(|path| format!(", resource_path: {path}"))
        .unwrap_or_default();
    let pid_status = if is_pid_dead(diagnostic.pid, diagnostic.pid_start_time_ticks) {
        "dead_or_recycled"
    } else {
        "alive"
    };
    format!(
        "Session locked by PID {} (lock_path: {}, pid_status: {}, \
         pid_start_time_ticks: {:?}, tool: {}, reason: {}, acquired: {}{}{})",
        diagnostic.pid,
        lock_path.display(),
        pid_status,
        diagnostic.pid_start_time_ticks,
        diagnostic.tool_name,
        diagnostic.reason,
        diagnostic.acquired_at,
        holder,
        resource
    )
}

/// Acquire a non-blocking exclusive lock for a session and tool.
///
/// Lock path: `{session_dir}/locks/{tool_name}.lock`
///
/// On success:
/// - Acquires exclusive advisory lock via `flock(2)` with `LOCK_NB`
/// - Writes diagnostic JSON (pid, tool_name, acquired_at, reason) to lock file
/// - Returns `SessionLock` guard that releases on drop
///
/// On failure:
/// - Attempts to read existing lock file to report which PID holds it
/// - Returns error with diagnostic information
pub fn acquire_lock(session_dir: &Path, tool_name: &str, reason: &str) -> Result<SessionLock> {
    let locks_dir = session_dir.join("locks");
    let lock_path = locks_dir.join(format!("{tool_name}.lock"));
    acquire_lock_at_path(&lock_path, tool_name, reason)
}

/// Acquire a per-parent fork-call serialization lock.
///
/// Lock path: `{state_root}/fork-call-parent-locks/<parent-session-id>.lock`
pub fn acquire_parent_fork_lock(
    state_root: &Path,
    parent_session_id: &str,
    reason: &str,
) -> Result<SessionLock> {
    let trimmed = parent_session_id.trim();
    if trimmed.is_empty() {
        anyhow::bail!("parent session id cannot be empty");
    }

    let safe_parent_id = sanitize_lock_component(trimmed);
    let lock_path = state_root
        .join("fork-call-parent-locks")
        .join(format!("{safe_parent_id}.lock"));
    let lock_name = format!("fork-call-parent:{trimmed}");
    acquire_lock_at_path(&lock_path, &lock_name, reason)
}

fn resolve_state_root() -> Result<PathBuf> {
    let base_dirs = directories::BaseDirs::new()
        .ok_or_else(|| anyhow::anyhow!("could not determine platform base directories"))?;
    Ok(base_dirs
        .state_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| base_dirs.data_local_dir().to_path_buf()))
}

/// Acquire a per-project resource lock for cross-session coordination.
///
/// Lock path:
/// `${XDG_STATE_HOME:-$HOME/.local/state}/cli-sub-agent/<resource_kind>-locks/<sha256(canonical(project_root))>/<tool_name>.lock`
///
/// Used when two CSA sessions in the same project_root must serialize on a
/// shared resource (e.g., the underlying jj or git repository).
pub fn acquire_project_resource_lock(
    project_root: &Path,
    resource_kind: &str,
    tool_name: &str,
    reason: &str,
) -> Result<SessionLock> {
    let (lock_path, lock_name, _) =
        project_resource_lock_path(project_root, resource_kind, tool_name)?;
    acquire_lock_at_path(&lock_path, &lock_name, reason)
}

pub(crate) fn project_resource_lock_path(
    project_root: &Path,
    resource_kind: &str,
    tool_name: &str,
) -> Result<(PathBuf, String, PathBuf)> {
    let kind = resource_kind.trim();
    if kind.is_empty() {
        anyhow::bail!("resource kind cannot be empty");
    }

    let tool = tool_name.trim();
    if tool.is_empty() {
        anyhow::bail!("tool name cannot be empty");
    }

    let canonical = fs::canonicalize(project_root).with_context(|| {
        format!(
            "csa-lock: failed to canonicalize project root '{}' \
             (cross-session lock coordination requires a stable canonical path); \
             ensure the path exists and is readable, and check sandbox config \
             (.csa/config.toml [filesystem_sandbox]) if running under bwrap/landlock",
            project_root.display()
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_os_str().as_encoded_bytes());
    let digest = format!("{:x}", hasher.finalize());
    let safe_kind = sanitize_lock_component(kind);
    let safe_tool = sanitize_lock_component(tool);
    let lock_path = resolve_state_root()?
        .join("cli-sub-agent")
        .join(format!("{safe_kind}-locks"))
        .join(digest)
        .join(format!("{safe_tool}.lock"));
    let lock_name = format!("{kind}:{tool}");
    Ok((lock_path, lock_name, canonical))
}

pub(crate) fn read_lock_diagnostic(lock_path: &Path) -> Result<Option<LockDiagnostic>> {
    let mut contents = String::new();
    File::open(lock_path)
        .with_context(|| format!("Failed to open lock file: {}", lock_path.display()))?
        .read_to_string(&mut contents)
        .with_context(|| format!("Failed to read lock file: {}", lock_path.display()))?;
    Ok(serde_json::from_str::<LockDiagnostic>(&contents).ok())
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
