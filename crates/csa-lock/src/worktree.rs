use crate::{
    SessionLock, acquire_lock_at_path_with_metadata, project_resource_lock_path,
    read_lock_diagnostic,
};
use anyhow::Result;
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// Guard for a write-capable CSA session sharing one git worktree.
///
/// Direct acquisitions own an exclusive `flock(2)` via [`SessionLock`].
/// Lineage re-entries are no-op guards used when a child session runs under
/// an ancestor session that already owns the process-level worktree lock.
pub struct WorktreeWriteLock {
    inner: WorktreeWriteLockKind,
    lock_path: PathBuf,
}

enum WorktreeWriteLockKind {
    Acquired {
        _lock: SessionLock,
        owner_session_id: String,
    },
    LineageReentry {
        holder_session_id: String,
    },
}

impl std::fmt::Debug for WorktreeWriteLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            WorktreeWriteLockKind::Acquired { .. } => f
                .debug_struct("WorktreeWriteLock")
                .field("mode", &"acquired")
                .field("lock_path", &self.lock_path)
                .finish(),
            WorktreeWriteLockKind::LineageReentry { holder_session_id } => f
                .debug_struct("WorktreeWriteLock")
                .field("mode", &"lineage_reentry")
                .field("holder_session_id", holder_session_id)
                .field("lock_path", &self.lock_path)
                .finish(),
        }
    }
}

impl WorktreeWriteLock {
    /// Returns the lock file path for this worktree write lock.
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Returns true when this session is allowed to proceed under an ancestor's lock.
    pub fn is_lineage_reentry(&self) -> bool {
        matches!(self.inner, WorktreeWriteLockKind::LineageReentry { .. })
    }

    /// Returns the ancestor session id that owns the underlying flock.
    pub fn holder_session_id(&self) -> Option<&str> {
        match &self.inner {
            WorktreeWriteLockKind::Acquired { .. } => None,
            WorktreeWriteLockKind::LineageReentry { holder_session_id } => {
                Some(holder_session_id.as_str())
            }
        }
    }
}

impl Drop for WorktreeWriteLock {
    fn drop(&mut self) {
        let WorktreeWriteLockKind::Acquired {
            owner_session_id, ..
        } = &self.inner
        else {
            return;
        };

        match read_lock_diagnostic(&self.lock_path) {
            Ok(Some(diagnostic))
                if diagnostic.holder_session_id.as_deref() == Some(owner_session_id.as_str()) =>
            {
                // Remove while the fd-level flock is still held so this drop
                // cannot remove a successor's freshly-created lock file.
                match fs::remove_file(&self.lock_path) {
                    Ok(()) => {}
                    Err(err) if err.kind() == ErrorKind::NotFound => {}
                    Err(err) => tracing::warn!(
                        lock_path = %self.lock_path.display(),
                        owner_session_id,
                        error = %err,
                        "failed to remove worktree write lock file on drop"
                    ),
                }
            }
            Ok(Some(diagnostic)) => tracing::warn!(
                lock_path = %self.lock_path.display(),
                owner_session_id,
                current_holder = diagnostic.holder_session_id.as_deref().unwrap_or("unknown"),
                "skipping worktree write lock removal because lock file holder changed"
            ),
            Ok(None) => tracing::warn!(
                lock_path = %self.lock_path.display(),
                owner_session_id,
                "skipping worktree write lock removal because diagnostic is unreadable"
            ),
            Err(err) => {
                if self.lock_path.exists() {
                    tracing::warn!(
                        lock_path = %self.lock_path.display(),
                        owner_session_id,
                        error = %err,
                        "skipping worktree write lock removal after diagnostic read failure"
                    );
                }
            }
        }
    }
}

/// Return true only when the canonical worktree write lock is actively held by
/// `session_id`.
///
/// This is a non-mutating liveness probe for wait/recovery paths: a stale
/// diagnostic file without an fd-level flock is not live, and a live flock whose
/// diagnostic names another session is not treated as this session's lock.
pub fn worktree_write_lock_is_held_by_session(
    worktree_root: &Path,
    session_id: &str,
) -> Result<bool> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        anyhow::bail!("session id cannot be empty");
    }

    let (lock_path, _lock_name, _canonical_root) =
        project_resource_lock_path(worktree_root, "worktree-write", "exclusive")?;
    if !lock_path.exists() {
        return Ok(false);
    }
    let diagnostic = match read_lock_diagnostic(&lock_path) {
        Ok(Some(diagnostic)) => diagnostic,
        Ok(None) => return Ok(false),
        Err(error) if !lock_path.exists() => {
            tracing::debug!(
                lock_path = %lock_path.display(),
                error = %error,
                "worktree lock disappeared during live-holder probe"
            );
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    if diagnostic.holder_session_id.as_deref() != Some(session_id) {
        return Ok(false);
    }

    let file = match OpenOptions::new().read(true).write(true).open(&lock_path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    // SAFETY: `file` owns a valid fd, and LOCK_EX | LOCK_NB is a non-blocking
    // advisory lock probe. If it succeeds, this process briefly acquired the
    // stale lock and immediately releases it before returning false.
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret == 0 {
        // SAFETY: same valid fd; release the probe lock before closing `file`.
        unsafe {
            libc::flock(file.as_raw_fd(), libc::LOCK_UN);
        }
        return Ok(false);
    }

    let error = std::io::Error::last_os_error();
    if error.kind() == ErrorKind::WouldBlock {
        return Ok(true);
    }
    Err(error.into())
}

/// Acquire a fail-fast exclusive write lock for a canonical git worktree root.
///
/// Lock path:
/// `${XDG_STATE_HOME:-$HOME/.local/state}/cli-sub-agent/worktree-write-locks/<sha256(canonical(worktree_root))>/exclusive.lock`
///
/// If the lock holder is one of `ancestor_session_ids`, the current session is
/// allowed to proceed only while that ancestor is still live. This prevents
/// fork-call child sessions from self-deadlocking while still blocking stale
/// lineage diagnostics that name dead ancestors.
///
/// `holder_session_is_live` must read caller-owned session state, not process
/// PID liveness; `csa-lock` stays independent of the session registry crate.
pub fn acquire_worktree_write_lock(
    worktree_root: &Path,
    session_id: &str,
    ancestor_session_ids: &[String],
    holder_session_is_live: impl Fn(&str) -> bool,
) -> Result<WorktreeWriteLock> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        anyhow::bail!("session id cannot be empty");
    }

    let (lock_path, lock_name, canonical_root) =
        project_resource_lock_path(worktree_root, "worktree-write", "exclusive")?;
    let reason = format!(
        "write session {session_id} holds worktree {}",
        canonical_root.display()
    );

    match acquire_lock_at_path_with_metadata(
        &lock_path,
        &lock_name,
        &reason,
        Some(session_id),
        Some(&canonical_root),
    ) {
        Ok(lock) => Ok(WorktreeWriteLock {
            inner: WorktreeWriteLockKind::Acquired {
                _lock: lock,
                owner_session_id: session_id.to_string(),
            },
            lock_path,
        }),
        Err(lock_error) => {
            let diagnostic = read_lock_diagnostic(&lock_path)?;
            if let Some(holder_session_id) = diagnostic
                .as_ref()
                .and_then(|diag| diag.holder_session_id.as_ref())
                && ancestor_session_ids
                    .iter()
                    .any(|ancestor| ancestor == holder_session_id)
                && holder_session_is_live(holder_session_id)
            {
                return Ok(WorktreeWriteLock {
                    inner: WorktreeWriteLockKind::LineageReentry {
                        holder_session_id: holder_session_id.clone(),
                    },
                    lock_path,
                });
            }

            // Stale lock recovery: if the holder PID is dead AND the flock is
            // actually free (no live process holds it), the lock file is stale
            // (the holder process died without running its Drop). Probe flock
            // liveness before reclaiming to avoid stealing a live unrelated
            // lock. (#2528)
            if let Some(diag) = diagnostic.as_ref()
                && is_pid_dead(diag.pid, diag.pid_start_time_ticks)
                && is_flock_available(&lock_path)
            {
                tracing::warn!(
                    lock_path = %lock_path.display(),
                    holder_pid = diag.pid,
                    holder_session = ?diag.holder_session_id,
                    "removing stale worktree write lock (holder PID is dead, flock is free)"
                );
                match acquire_lock_at_path_with_metadata(
                    &lock_path,
                    &lock_name,
                    &reason,
                    Some(session_id),
                    Some(&canonical_root),
                ) {
                    Ok(lock) => {
                        return Ok(WorktreeWriteLock {
                            inner: WorktreeWriteLockKind::Acquired {
                                _lock: lock,
                                owner_session_id: session_id.to_string(),
                            },
                            lock_path,
                        });
                    }
                    Err(retry_error) => {
                        anyhow::bail!(
                            "concurrent write session blocked: stale lock detected but retry \
                             failed: {}. {}",
                            retry_error,
                            lock_error
                        );
                    }
                }
            }

            let holder = diagnostic
                .as_ref()
                .and_then(|diag| diag.holder_session_id.as_deref())
                .unwrap_or("unknown");
            anyhow::bail!(
                "concurrent write session blocked: worktree '{}' is already locked by session {}. \
                 {}. Run write-capable CSA sessions for this repository sequentially; \
                 wait for the holder to finish or stop it before starting another writer.",
                canonical_root.display(),
                holder,
                lock_error
            )
        }
    }
}

/// Check whether a PID is dead by sending signal 0 (no-op probe).
/// Returns true if the PID does not exist or its start time no longer
/// matches (recycled PID). On platforms where start-time detection is
/// unavailable, uses only the signal-0 probe.
fn is_pid_dead(pid: u32, pid_start_time_ticks: Option<u64>) -> bool {
    // SAFETY: `kill(pid, 0)` sends no signal; it only checks existence.
    let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
    if !alive {
        // ESRCH = no such process
        return true;
    }
    // Process exists — check if PID was recycled by comparing start time.
    if let Some(expected_ticks) = pid_start_time_ticks
        && let Some(current_ticks) = crate::process_start_time_ticks(pid)
    {
        return current_ticks != expected_ticks;
    }
    // Can't verify start time; assume the process is alive (conservative).
    false
}

/// Probe whether the flock on `lock_path` is actually available (not held
/// by any live process). Opens the file read-write and attempts a
/// non-blocking exclusive lock. If it succeeds, the lock is free; we
/// immediately release it so the caller can retry the normal acquisition.
fn is_flock_available(lock_path: &Path) -> bool {
    let file = match OpenOptions::new().read(true).write(true).open(lock_path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    // SAFETY: `file` owns a valid fd; LOCK_EX | LOCK_NB is non-blocking.
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret == 0 {
        // SAFETY: same valid fd; release the probe lock.
        unsafe {
            libc::flock(file.as_raw_fd(), libc::LOCK_UN);
        }
        true
    } else {
        false
    }
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
