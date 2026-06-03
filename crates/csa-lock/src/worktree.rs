use crate::{
    SessionLock, acquire_lock_at_path_with_metadata, project_resource_lock_path,
    read_lock_diagnostic,
};
use anyhow::Result;
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
    Acquired { _lock: SessionLock },
    LineageReentry { holder_session_id: String },
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
            inner: WorktreeWriteLockKind::Acquired { _lock: lock },
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use tempfile::tempdir;

    fn env_test_lock() -> MutexGuard<'static, ()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set_os(key: &'static str, value: &Path) -> Self {
            let original = std::env::var_os(key);
            // SAFETY: test-scoped env mutation is isolated to the current test.
            unsafe { std::env::set_var(key, value) };
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.original.take() {
                // SAFETY: test-scoped env restoration is isolated to the current test.
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                // SAFETY: test-scoped env restoration is isolated to the current test.
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn test_worktree_write_lock_conflicts_for_same_worktree_root() {
        let _env_lock = env_test_lock();
        let state_home = tempdir().expect("state-home tempdir");
        let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

        let worktree_root = tempdir().expect("worktree tempdir");
        let _lock1 = acquire_worktree_write_lock(worktree_root.path(), "01PARENT", &[])
            .expect("first worktree write lock should succeed");

        let err = acquire_worktree_write_lock(worktree_root.path(), "01OTHER", &[])
            .expect_err("non-lineage writer should fail fast")
            .to_string();

        assert!(err.contains("01PARENT"), "missing holder session id: {err}");
        assert!(
            err.contains(&worktree_root.path().display().to_string()),
            "missing worktree path: {err}"
        );
        assert!(
            err.contains("sequentially"),
            "missing serialize guidance: {err}"
        );
    }

    #[test]
    fn test_worktree_write_lock_allows_lineage_reentry() {
        let _env_lock = env_test_lock();
        let state_home = tempdir().expect("state-home tempdir");
        let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

        let worktree_root = tempdir().expect("worktree tempdir");
        let _parent = acquire_worktree_write_lock(worktree_root.path(), "01PARENT", &[])
            .expect("parent worktree write lock should succeed");

        let child =
            acquire_worktree_write_lock(worktree_root.path(), "01CHILD", &["01PARENT".to_string()])
                .expect("child should re-enter under ancestor lock");

        assert!(child.is_lineage_reentry());
        assert_eq!(child.holder_session_id(), Some("01PARENT"));
    }

    #[test]
    fn test_worktree_write_lock_allows_different_worktree_roots() {
        let _env_lock = env_test_lock();
        let state_home = tempdir().expect("state-home tempdir");
        let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

        let worktree_a = tempdir().expect("worktree a tempdir");
        let worktree_b = tempdir().expect("worktree b tempdir");

        let lock_a = acquire_worktree_write_lock(worktree_a.path(), "01A", &[]);
        let lock_b = acquire_worktree_write_lock(worktree_b.path(), "01B", &[]);

        assert!(lock_a.is_ok());
        assert!(lock_b.is_ok());
    }
}
