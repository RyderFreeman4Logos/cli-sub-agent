use crate::{
    SessionLock, acquire_lock_at_path_with_metadata, project_resource_lock_path,
    read_lock_diagnostic,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::fs;
use std::io::ErrorKind;
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

/// Session-registry liveness for the holder recorded in a worktree lock file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HolderSessionLiveness {
    /// The holder session is still running or otherwise registered as live.
    Live,
    /// The holder session is positively known to be completed or retired.
    Dead,
    /// The holder session was not found in the registry; diagnostic PID proof is still required.
    RegistryAbsent,
    /// The caller could not determine holder-session state.
    Unknown,
}

impl HolderSessionLiveness {
    /// Construct a live holder-session state for external liveness callbacks.
    pub const fn live() -> Self {
        Self::Live
    }

    /// Construct a positively dead holder-session state for external liveness callbacks.
    pub const fn dead() -> Self {
        Self::Dead
    }

    /// Construct a registry-absent holder-session state for external liveness callbacks.
    pub const fn registry_absent() -> Self {
        Self::RegistryAbsent
    }

    /// Construct an unknown holder-session state for external liveness callbacks.
    pub const fn unknown() -> Self {
        Self::Unknown
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
    acquire_worktree_write_lock_with_liveness(
        worktree_root,
        session_id,
        ancestor_session_ids,
        |_| HolderSessionLiveness::unknown(),
    )
}

/// Acquire a worktree write lock, reclaiming a stale lock only when the holder
/// session is known dead by the caller-provided registry check.
pub fn acquire_worktree_write_lock_with_liveness(
    worktree_root: &Path,
    session_id: &str,
    ancestor_session_ids: &[String],
    holder_session_liveness: impl FnMut(&str) -> HolderSessionLiveness,
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
            {
                return Ok(WorktreeWriteLock {
                    inner: WorktreeWriteLockKind::LineageReentry {
                        holder_session_id: holder_session_id.clone(),
                    },
                    lock_path,
                });
            }

            if let Some(lock) = try_reclaim_stale_worktree_lock(
                &lock_path,
                &lock_name,
                &reason,
                session_id,
                &canonical_root,
                diagnostic.as_ref(),
                holder_session_liveness,
            )? {
                return Ok(WorktreeWriteLock {
                    inner: WorktreeWriteLockKind::Acquired {
                        _lock: lock,
                        owner_session_id: session_id.to_string(),
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

fn try_reclaim_stale_worktree_lock(
    lock_path: &Path,
    lock_name: &str,
    reason: &str,
    session_id: &str,
    canonical_root: &Path,
    diagnostic: Option<&crate::LockDiagnostic>,
    mut holder_session_liveness: impl FnMut(&str) -> HolderSessionLiveness,
) -> Result<Option<SessionLock>> {
    let Some(diagnostic) = diagnostic else {
        return Ok(None);
    };
    let Some(holder_session_id) = diagnostic.holder_session_id.as_deref() else {
        return Ok(None);
    };

    if !stale_holder_can_be_reclaimed(diagnostic, holder_session_id, &mut holder_session_liveness) {
        return Ok(None);
    }

    let reclaim_lock_path = lock_path.with_file_name(".reclaim.lock");
    let _reclaim_guard = match acquire_lock_at_path_with_metadata(
        &reclaim_lock_path,
        "worktree-write:reclaim",
        "serialize stale worktree lock reclaim",
        Some(session_id),
        Some(canonical_root),
    ) {
        Ok(guard) => guard,
        Err(err) => {
            tracing::warn!(
                lock_path = %lock_path.display(),
                holder_session_id,
                error = %err,
                "failed to acquire worktree stale-lock reclaim guard; keeping existing lock"
            );
            return Ok(None);
        }
    };

    let current = read_lock_diagnostic(lock_path)?;
    let Some(current) = current else {
        return Ok(None);
    };
    if !same_lock_holder(&current, diagnostic) {
        return Ok(None);
    }

    let stale_path = stale_lock_path(lock_path, diagnostic.acquired_at);
    fs::rename(lock_path, &stale_path).with_context(|| {
        format!(
            "failed to atomically move stale worktree lock '{}' aside",
            lock_path.display()
        )
    })?;

    tracing::warn!(
        lock_path = %lock_path.display(),
        stale_path = %stale_path.display(),
        old_holder_pid = diagnostic.pid,
        old_holder_session_id = holder_session_id,
        old_acquired_at = %diagnostic.acquired_at,
        "reclaimed stale worktree write lock"
    );

    let lock = match acquire_lock_at_path_with_metadata(
        lock_path,
        lock_name,
        reason,
        Some(session_id),
        Some(canonical_root),
    ) {
        Ok(lock) => lock,
        Err(err) => {
            if let Err(restore_err) = fs::rename(&stale_path, lock_path)
                && restore_err.kind() != ErrorKind::NotFound
            {
                tracing::warn!(
                    lock_path = %lock_path.display(),
                    stale_path = %stale_path.display(),
                    error = %restore_err,
                    "failed to restore stale worktree lock after reclaim acquisition error"
                );
            }
            return Err(err).with_context(|| {
                format!(
                    "failed to acquire worktree write lock after reclaiming stale holder {holder_session_id}"
                )
            });
        }
    };
    if let Err(err) = fs::remove_file(&stale_path)
        && err.kind() != ErrorKind::NotFound
    {
        tracing::warn!(
            stale_path = %stale_path.display(),
            error = %err,
            "failed to remove reclaimed stale worktree lock artifact"
        );
    }
    Ok(Some(lock))
}

fn stale_holder_can_be_reclaimed(
    diagnostic: &crate::LockDiagnostic,
    holder_session_id: &str,
    holder_session_liveness: &mut impl FnMut(&str) -> HolderSessionLiveness,
) -> bool {
    match holder_session_liveness(holder_session_id) {
        HolderSessionLiveness::Live | HolderSessionLiveness::Unknown => return false,
        HolderSessionLiveness::RegistryAbsent => {
            return matches!(
                process_probe_state(diagnostic.pid),
                ProcessProbeState::Missing
            );
        }
        HolderSessionLiveness::Dead => {}
    }

    // Loaded terminal session state is stronger than a signalable diagnostic
    // PID because the numeric PID may have been reused after the CSA holder
    // exited. EPERM or probe failure stays conservative because we cannot
    // inspect enough to rule out the holder.
    !matches!(
        process_probe_state(diagnostic.pid),
        ProcessProbeState::PermissionDenied | ProcessProbeState::Unknown
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessProbeState {
    Missing,
    Signalable,
    PermissionDenied,
    Unknown,
}

fn process_probe_state(pid: u32) -> ProcessProbeState {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 checks process existence without sending a signal.
        let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if ret == 0 {
            return ProcessProbeState::Signalable;
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => ProcessProbeState::Missing,
            Some(libc::EPERM) => ProcessProbeState::PermissionDenied,
            _ => ProcessProbeState::Unknown,
        }
    }

    #[cfg(not(unix))]
    {
        let _ = pid;
        ProcessProbeState::Unknown
    }
}

fn same_lock_holder(left: &crate::LockDiagnostic, right: &crate::LockDiagnostic) -> bool {
    left.pid == right.pid
        && left.holder_session_id == right.holder_session_id
        && left.acquired_at == right.acquired_at
        && left.resource_path == right.resource_path
}

fn stale_lock_path(lock_path: &Path, acquired_at: DateTime<Utc>) -> PathBuf {
    let suffix = acquired_at
        .timestamp_nanos_opt()
        .unwrap_or_else(|| Utc::now().timestamp_nanos_opt().unwrap_or_default());
    lock_path.with_file_name(format!(".exclusive.stale.{suffix}.lock"))
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
