use crate::{
    SessionLock, acquire_lock_at_path_with_metadata, project_resource_lock_path,
    read_lock_diagnostic,
};
use anyhow::Result;
use std::fs;
use std::io::ErrorKind;
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
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

/// Acquire a fail-fast exclusive write lock for a canonical git worktree root.
///
/// Lock path:
/// `${XDG_STATE_HOME:-$HOME/.local/state}/cli-sub-agent/worktree-write-locks/<sha256(canonical(worktree_root))>/exclusive.lock`
///
/// If the lock holder is one of `ancestor_session_ids`, the current session is
/// allowed to proceed under the ancestor's flock. This prevents fork-call child
/// sessions from self-deadlocking while still blocking unrelated write sessions.
pub fn acquire_worktree_write_lock(
    worktree_root: &Path,
    session_id: &str,
    ancestor_session_ids: &[String],
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
                && diagnostic
                    .as_ref()
                    .is_some_and(|diag| diagnostic_matches_current_flock_owner(&lock_path, diag))
            {
                return Ok(WorktreeWriteLock {
                    inner: WorktreeWriteLockKind::LineageReentry {
                        holder_session_id: holder_session_id.clone(),
                    },
                    lock_path,
                });
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

fn diagnostic_matches_current_flock_owner(
    lock_path: &Path,
    diagnostic: &crate::LockDiagnostic,
) -> bool {
    let Some(owner_pid) = current_flock_owner_pid(lock_path) else {
        return false;
    };
    owner_pid == diagnostic.pid && diagnostic_process_identity_matches(diagnostic)
}

#[cfg(target_os = "linux")]
fn current_flock_owner_pid(lock_path: &Path) -> Option<u32> {
    let metadata = fs::metadata(lock_path).ok()?;
    let (major, minor) = linux_dev_major_minor(metadata.dev());
    let inode = metadata.ino();
    fs::read_to_string("/proc/locks")
        .ok()?
        .lines()
        .find_map(|line| proc_lock_owner_for_inode(line, major, minor, inode))
}

#[cfg(target_os = "linux")]
fn proc_lock_owner_for_inode(line: &str, major: u64, minor: u64, inode: u64) -> Option<u32> {
    let mut fields = line.split_whitespace();
    let _id = fields.next()?;
    if fields.next()? != "FLOCK" {
        return None;
    }
    let _scope = fields.next()?;
    if fields.next()? != "WRITE" {
        return None;
    }
    let pid = fields.next()?.parse::<u32>().ok()?;
    let lock_id = fields.next()?;
    let mut parts = lock_id.split(':');
    let lock_major = u64::from_str_radix(parts.next()?, 16).ok()?;
    let lock_minor = u64::from_str_radix(parts.next()?, 16).ok()?;
    let lock_inode = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    (lock_major == major && lock_minor == minor && lock_inode == inode).then_some(pid)
}

#[cfg(target_os = "linux")]
fn linux_dev_major_minor(dev: u64) -> (u64, u64) {
    let major = ((dev >> 8) & 0x0fff) | ((dev >> 32) & !0x0fff);
    let minor = (dev & 0x00ff) | ((dev >> 12) & !0x00ff);
    (major, minor)
}

#[cfg(target_os = "linux")]
fn diagnostic_process_identity_matches(diagnostic: &crate::LockDiagnostic) -> bool {
    let Some(expected_start_time) = diagnostic.pid_start_time_ticks else {
        return false;
    };
    crate::process_start_time_ticks(diagnostic.pid) == Some(expected_start_time)
}

#[cfg(not(target_os = "linux"))]
fn current_flock_owner_pid(_lock_path: &Path) -> Option<u32> {
    Some(std::process::id())
}

#[cfg(not(target_os = "linux"))]
fn diagnostic_process_identity_matches(diagnostic: &crate::LockDiagnostic) -> bool {
    diagnostic.pid == std::process::id()
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
