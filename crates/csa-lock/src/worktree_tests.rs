use super::*;
use crate::LockDiagnostic;
use chrono::Utc;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::os::unix::io::AsRawFd;
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
fn worktree_write_lock_acquires_over_stale_file_without_held_flock() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let worktree_root = tempdir().expect("worktree tempdir");
    let holder = acquire_worktree_write_lock(worktree_root.path(), "01OLDOWNER", &[])
        .expect("holder lock should succeed");
    let lock_path = holder.lock_path().to_path_buf();
    overwrite_worktree_lock_diagnostic(
        &lock_path,
        std::process::id(),
        crate::process_start_time_ticks(std::process::id()),
        "01STALE",
        worktree_root.path(),
    );
    drop(holder);

    let lock = acquire_worktree_write_lock(worktree_root.path(), "01NEXT", &[])
        .expect("stale diagnostic file without a held flock must not block acquisition");

    assert!(!lock.is_lineage_reentry());
    let diagnostic = read_lock_diagnostic(&lock_path)
        .expect("read refreshed diagnostic")
        .expect("refreshed diagnostic should parse");
    assert_eq!(diagnostic.holder_session_id.as_deref(), Some("01NEXT"));
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

    let err = acquire_worktree_write_lock(worktree_root.path(), "01NEXT", &[])
        .expect_err("live holder must still block")
        .to_string();

    assert!(err.contains("concurrent write session blocked"));
    assert!(err.contains("01LIVE"));
}

#[test]
fn worktree_write_lock_blocks_post_exec_window_with_live_flock() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let worktree_root = tempdir().expect("worktree tempdir");
    let holder = acquire_worktree_write_lock(worktree_root.path(), "01DONE", &[])
        .expect("holder lock should succeed");
    let lock_path = holder.lock_path().to_path_buf();
    overwrite_worktree_lock_diagnostic(
        &lock_path,
        std::process::id(),
        crate::process_start_time_ticks(std::process::id()),
        "01DONE",
        worktree_root.path(),
    );

    let err = acquire_worktree_write_lock(worktree_root.path(), "01NEXT", &[])
        .expect_err("post-exec holder with live flock must block")
        .to_string();

    assert!(err.contains("concurrent write session blocked"));
    assert!(err.contains("01DONE"));
    assert!(
        lock_path.exists(),
        "canonical lock path must stay in place while the live flock is held"
    );
}

#[test]
fn worktree_write_lock_blocks_stale_diagnostic_with_live_unrelated_flock_without_stealing_inode() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let worktree_root = tempdir().expect("worktree tempdir");
    let stale_holder = acquire_worktree_write_lock(worktree_root.path(), "01OLDOWNER", &[])
        .expect("stale holder setup lock should succeed");
    let lock_path = stale_holder.lock_path().to_path_buf();
    overwrite_worktree_lock_diagnostic(&lock_path, 4_000_000, None, "01DEAD", worktree_root.path());
    drop(stale_holder);
    let before_inode = fs::metadata(&lock_path).expect("lock metadata").ino();
    let _manual_holder = ManualFlock::acquire(&lock_path);

    let err = acquire_worktree_write_lock(worktree_root.path(), "01NEXT", &[])
        .expect_err("live unrelated flock must block despite stale diagnostic")
        .to_string();

    assert!(err.contains("concurrent write session blocked"));
    assert!(err.contains("01DEAD"));
    assert!(
        lock_path.exists(),
        "canonical lock path must not be moved aside"
    );
    assert_eq!(
        fs::metadata(&lock_path).expect("lock metadata").ino(),
        before_inode,
        "canonical lock inode must not be replaced"
    );
    assert_no_reclaim_artifacts(&lock_path);
}

#[test]
fn worktree_write_lock_blocks_lineage_reentry_when_stale_diagnostic_is_not_flock_owner() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let worktree_root = tempdir().expect("worktree tempdir");
    let stale_holder = acquire_worktree_write_lock(worktree_root.path(), "01OLDOWNER", &[])
        .expect("stale holder setup lock should succeed");
    let lock_path = stale_holder.lock_path().to_path_buf();
    overwrite_worktree_lock_diagnostic(
        &lock_path,
        std::process::id(),
        Some(0),
        "01DEAD",
        worktree_root.path(),
    );
    drop(stale_holder);
    let before_inode = fs::metadata(&lock_path).expect("lock metadata").ino();
    let _manual_holder = ManualFlock::acquire(&lock_path);

    let err = acquire_worktree_write_lock(
        worktree_root.path(),
        "01DESCENDANT",
        &["01DEAD".to_string()],
    )
    .expect_err("stale ancestor diagnostic must not bypass a live unrelated flock")
    .to_string();

    assert!(err.contains("concurrent write session blocked"));
    assert!(err.contains("01DEAD"));
    assert_eq!(
        fs::metadata(&lock_path).expect("lock metadata").ino(),
        before_inode,
        "canonical lock inode must not be replaced"
    );
    assert_no_reclaim_artifacts(&lock_path);
}

fn overwrite_worktree_lock_diagnostic(
    lock_path: &Path,
    pid: u32,
    pid_start_time_ticks: Option<u64>,
    session_id: &str,
    worktree_root: &Path,
) {
    let diagnostic = LockDiagnostic {
        pid,
        pid_start_time_ticks,
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

struct ManualFlock {
    file: File,
}

impl ManualFlock {
    fn acquire(lock_path: &Path) -> Self {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path)
            .expect("open canonical lock file");
        // SAFETY: `file` owns a valid fd, and LOCK_EX requests an advisory flock.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(ret, 0, "manual flock setup should acquire the lock");
        Self { file }
    }
}

impl Drop for ManualFlock {
    fn drop(&mut self) {
        // SAFETY: `file` owns a valid fd; unlocking before close is deterministic cleanup.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn assert_no_reclaim_artifacts(lock_path: &Path) {
    let lock_dir = lock_path.parent().expect("lock path should have parent");
    for entry in fs::read_dir(lock_dir).expect("read lock dir") {
        let entry = entry.expect("read lock dir entry");
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        assert_ne!(
            file_name, ".reclaim.lock",
            "reclaim guard must not be created"
        );
        assert!(
            !file_name.starts_with(".exclusive.stale."),
            "stale lock artifact must not be created: {file_name}"
        );
    }
}
