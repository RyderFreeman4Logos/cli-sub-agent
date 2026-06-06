use super::*;
use crate::LockDiagnostic;
use chrono::Utc;
use std::ffi::OsString;
use std::fs;
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

#[test]
fn worktree_write_lock_removes_lock_file_on_drop() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let worktree_root = tempdir().expect("worktree tempdir");
    let lock = acquire_worktree_write_lock(worktree_root.path(), "01OWNER", &[])
        .expect("worktree write lock should succeed");
    let lock_path = lock.lock_path().to_path_buf();
    assert!(lock_path.exists(), "lock file should exist while held");

    drop(lock);

    assert!(
        !lock_path.exists(),
        "owned worktree lock file should be removed when guard drops"
    );
    acquire_worktree_write_lock(worktree_root.path(), "01NEXT", &[])
        .expect("next writer should acquire after guard drop");
}

#[test]
fn worktree_write_lock_reclaims_dead_holder_lock() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let worktree_root = tempdir().expect("worktree tempdir");
    let holder = acquire_worktree_write_lock(worktree_root.path(), "01DEAD", &[])
        .expect("holder lock should succeed");
    overwrite_worktree_lock_diagnostic(
        holder.lock_path(),
        missing_pid(),
        "01DEAD",
        worktree_root.path(),
    );

    let lock =
        acquire_worktree_write_lock_with_liveness(worktree_root.path(), "01NEXT", &[], |_| {
            HolderSessionLiveness::Dead
        })
        .expect("dead holder lock should be reclaimed");

    assert!(!lock.is_lineage_reentry());
    drop(holder);
    drop(lock);
}

#[test]
fn worktree_write_lock_keeps_live_holder_blocked() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let worktree_root = tempdir().expect("worktree tempdir");
    let _holder = acquire_worktree_write_lock(worktree_root.path(), "01LIVE", &[])
        .expect("holder lock should succeed");

    let err =
        acquire_worktree_write_lock_with_liveness(worktree_root.path(), "01NEXT", &[], |_| {
            HolderSessionLiveness::Live
        })
        .expect_err("live holder must still block")
        .to_string();

    assert!(err.contains("concurrent write session blocked"));
    assert!(err.contains("01LIVE"));
}

#[test]
fn worktree_write_lock_keeps_registry_absent_signalable_holder_blocked() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let worktree_root = tempdir().expect("worktree tempdir");
    let holder = acquire_worktree_write_lock(worktree_root.path(), "01ABSENT", &[])
        .expect("holder lock should succeed");
    let lock_path = holder.lock_path().to_path_buf();
    overwrite_worktree_lock_diagnostic(
        &lock_path,
        std::process::id(),
        "01ABSENT",
        worktree_root.path(),
    );

    let err =
        acquire_worktree_write_lock_with_liveness(worktree_root.path(), "01NEXT", &[], |_| {
            HolderSessionLiveness::RegistryAbsent
        })
        .expect_err("registry-absent holder with signalable pid must block")
        .to_string();

    assert!(err.contains("concurrent write session blocked"));
    assert!(err.contains("01ABSENT"));
    assert!(
        lock_path.exists(),
        "canonical lock path must not be moved aside"
    );
    let diagnostic = read_lock_diagnostic(&lock_path)
        .expect("read retained diagnostic")
        .expect("retained diagnostic should parse");
    assert_eq!(diagnostic.holder_session_id.as_deref(), Some("01ABSENT"));
}

#[test]
fn worktree_write_lock_keeps_unknown_signalable_holder_blocked() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let worktree_root = tempdir().expect("worktree tempdir");
    let holder = acquire_worktree_write_lock(worktree_root.path(), "01UNKNOWN", &[])
        .expect("holder lock should succeed");
    let lock_path = holder.lock_path().to_path_buf();
    overwrite_worktree_lock_diagnostic(
        &lock_path,
        std::process::id(),
        "01UNKNOWN",
        worktree_root.path(),
    );

    let err =
        acquire_worktree_write_lock_with_liveness(worktree_root.path(), "01NEXT", &[], |_| {
            HolderSessionLiveness::Unknown
        })
        .expect_err("unknown holder liveness must block")
        .to_string();

    assert!(err.contains("concurrent write session blocked"));
    assert!(err.contains("01UNKNOWN"));
    assert!(
        lock_path.exists(),
        "canonical lock path must not be moved aside"
    );
}

#[test]
fn worktree_write_lock_reclaims_registry_absent_holder_with_missing_pid() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let worktree_root = tempdir().expect("worktree tempdir");
    let holder = acquire_worktree_write_lock(worktree_root.path(), "01ABSENT", &[])
        .expect("holder lock should succeed");
    overwrite_worktree_lock_diagnostic(
        holder.lock_path(),
        missing_pid(),
        "01ABSENT",
        worktree_root.path(),
    );

    let lock =
        acquire_worktree_write_lock_with_liveness(worktree_root.path(), "01NEXT", &[], |_| {
            HolderSessionLiveness::RegistryAbsent
        })
        .expect("registry-absent holder with missing pid should be reclaimed");

    assert!(!lock.is_lineage_reentry());
    drop(holder);
    drop(lock);
}

fn overwrite_worktree_lock_diagnostic(
    lock_path: &Path,
    pid: u32,
    session_id: &str,
    worktree_root: &Path,
) {
    let diagnostic = LockDiagnostic {
        pid,
        tool_name: "worktree-write:exclusive".to_string(),
        acquired_at: Utc::now(),
        reason: format!(
            "write session {session_id} holds worktree {}",
            worktree_root.display()
        ),
        holder_session_id: Some(session_id.to_string()),
        resource_path: Some(worktree_root.display().to_string()),
    };
    fs::write(lock_path, serde_json::to_string(&diagnostic).unwrap())
        .expect("overwrite lock diagnostic");
}

fn missing_pid() -> u32 {
    [4_000_000, 8_000_000, 16_000_000, 1_000_000_000]
        .into_iter()
        .find(|pid| matches!(process_probe_state(*pid), ProcessProbeState::Missing))
        .expect("test host should have at least one definitely missing pid")
}
