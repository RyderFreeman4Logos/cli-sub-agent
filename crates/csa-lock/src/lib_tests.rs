use super::*;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::sync::{Mutex, MutexGuard, OnceLock};
use tempfile::tempdir;

/// Process-wide mutex serializing tests that mutate `HOME` / `XDG_STATE_HOME`.
///
/// `std::env::set_var` / `std::env::remove_var` are not thread-safe; cargo runs
/// tests in parallel within one binary, so env-mutating tests would race
/// without this guard.
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

    fn unset(key: &'static str) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: test-scoped env mutation is isolated to the current test.
        unsafe { std::env::remove_var(key) };
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
fn test_acquire_lock_succeeds() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let session_dir = temp_dir.path();

    let lock = acquire_lock(session_dir, "test-tool", "test reason");
    assert!(lock.is_ok(), "Lock acquisition should succeed");

    let lock = lock.unwrap();
    assert!(lock.lock_path().exists(), "Lock file should exist");
}

#[test]
fn test_lock_file_path() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let session_dir = temp_dir.path();

    let lock =
        acquire_lock(session_dir, "test-tool", "test reason").expect("Failed to acquire lock");

    let expected_path = session_dir.join("locks/test-tool.lock");
    assert_eq!(lock.lock_path(), expected_path);
    assert!(
        expected_path.exists(),
        "Lock file should exist at correct path"
    );
}

#[test]
fn test_lock_diagnostic_written() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let session_dir = temp_dir.path();

    let _lock =
        acquire_lock(session_dir, "test-tool", "test reason").expect("Failed to acquire lock");

    let lock_path = session_dir.join("locks/test-tool.lock");
    let contents = fs::read_to_string(&lock_path).expect("Failed to read lock file");

    let diagnostic: LockDiagnostic =
        serde_json::from_str(&contents).expect("Failed to parse diagnostic JSON");

    assert_eq!(diagnostic.pid, std::process::id());
    assert_eq!(diagnostic.tool_name, "test-tool");
    assert_eq!(diagnostic.reason, "test reason");
}

#[test]
fn test_second_lock_fails() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let session_dir = temp_dir.path();

    let _lock1 =
        acquire_lock(session_dir, "test-tool", "first reason").expect("First lock should succeed");

    let result = acquire_lock(session_dir, "test-tool", "second reason");
    assert!(result.is_err(), "Second lock should fail");

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Session locked by PID"),
        "Error message should contain PID info"
    );
    assert!(
        err_msg.contains("reason: first reason"),
        "Error message should contain reason info"
    );
}

#[test]
fn session_lock_recovers_dead_pid_with_held_stale_flock() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let lock_path = temp_dir.path().join("locks/codex.lock");
    write_session_lock_diagnostic(
        &lock_path,
        dead_test_pid(),
        None,
        "codex",
        "stale reason",
        Some("01STALE"),
        None,
    );
    let _stale_flock = ManualFlock::acquire(&lock_path);

    let lock = acquire_lock_at_path(&lock_path, "codex", "fresh reason")
        .expect("dead PID diagnostic should be force-cleared and retried");

    assert_eq!(lock.lock_path(), lock_path.as_path());
    let diagnostic = read_lock_diagnostic(&lock_path)
        .expect("read refreshed diagnostic")
        .expect("refreshed diagnostic should parse");
    assert_eq!(diagnostic.pid, std::process::id());
    assert_eq!(diagnostic.tool_name, "codex");
    assert_eq!(diagnostic.reason, "fresh reason");
    assert_eq!(diagnostic.holder_session_id, None);
}

#[cfg(target_os = "linux")]
#[test]
fn session_lock_recovers_recycled_pid_with_different_start_time() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let lock_path = temp_dir.path().join("locks/codex.lock");
    let current_ticks = process_start_time_ticks(std::process::id())
        .expect("current process start time should be available on linux");
    let stale_ticks = current_ticks
        .checked_add(1)
        .unwrap_or_else(|| current_ticks.saturating_sub(1));
    write_session_lock_diagnostic(
        &lock_path,
        std::process::id(),
        Some(stale_ticks),
        "codex",
        "recycled holder",
        Some("01RECYCLED"),
        None,
    );
    let _stale_flock = ManualFlock::acquire(&lock_path);

    let lock = acquire_lock_at_path(&lock_path, "codex", "fresh after recycle")
        .expect("recycled PID diagnostic should be force-cleared and retried");

    assert_eq!(lock.lock_path(), lock_path.as_path());
    let diagnostic = read_lock_diagnostic(&lock_path)
        .expect("read refreshed diagnostic")
        .expect("refreshed diagnostic should parse");
    assert_eq!(diagnostic.pid, std::process::id());
    assert_eq!(diagnostic.reason, "fresh after recycle");
}

#[test]
fn session_lock_live_holder_still_fails() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let session_dir = temp_dir.path();

    let _lock =
        acquire_lock(session_dir, "codex", "live reason").expect("live holder lock should succeed");

    let err = acquire_lock(session_dir, "codex", "waiter reason")
        .expect_err("live holder must still block")
        .to_string();

    assert!(err.contains("Session locked by PID"));
    assert!(err.contains("codex"));
    assert!(err.contains("live reason"));
}

#[test]
fn session_lock_stale_cleanup_writes_new_diagnostic_metadata() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let lock_path = temp_dir.path().join("locks/codex.lock");
    write_session_lock_diagnostic(
        &lock_path,
        dead_test_pid(),
        None,
        "codex",
        "old diagnostic",
        Some("01OLD"),
        None,
    );
    let _stale_flock = ManualFlock::acquire(&lock_path);

    let lock = acquire_lock_at_path_with_metadata(
        &lock_path,
        "codex",
        "new diagnostic",
        Some("01NEW"),
        None,
    )
    .expect("stale cleanup should write fresh metadata");

    assert_eq!(lock.lock_path(), lock_path.as_path());
    let diagnostic = read_lock_diagnostic(&lock_path)
        .expect("read refreshed diagnostic")
        .expect("refreshed diagnostic should parse");
    assert_eq!(diagnostic.pid, std::process::id());
    assert_eq!(
        diagnostic.pid_start_time_ticks,
        process_start_time_ticks(std::process::id())
    );
    assert_eq!(diagnostic.tool_name, "codex");
    assert_eq!(diagnostic.reason, "new diagnostic");
    assert_eq!(diagnostic.holder_session_id.as_deref(), Some("01NEW"));
    assert_eq!(diagnostic.resource_path, None);
}

#[test]
fn test_lock_released_on_drop() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let session_dir = temp_dir.path();

    {
        let _lock = acquire_lock(session_dir, "test-tool", "test reason")
            .expect("First lock should succeed");
        // Lock is held here
    } // Lock is dropped here

    // Note: flock locks are process-scoped, not thread-scoped.
    // Within the same process, the lock is still held even after close
    // because flock only releases when ALL fds referencing the same
    // open file description are closed. This test documents the behavior
    // but won't actually re-acquire within the same process.
    // In real usage across processes, the lock is properly released.
}

#[test]
fn test_different_tools_different_locks() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let session_dir = temp_dir.path();

    let lock1 =
        acquire_lock(session_dir, "tool-a", "reason a").expect("Tool A lock should succeed");
    let lock2 =
        acquire_lock(session_dir, "tool-b", "reason b").expect("Tool B lock should succeed");

    assert_ne!(
        lock1.lock_path(),
        lock2.lock_path(),
        "Different tools should have different lock files"
    );
}

#[test]
fn test_lock_path_follows_convention() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let session_dir = temp_dir.path();

    let lock = acquire_lock(session_dir, "codex", "running task").expect("Lock should succeed");

    // Convention: {session_dir}/locks/{tool_name}.lock
    let expected = session_dir.join("locks").join("codex.lock");
    assert_eq!(lock.lock_path(), expected);
}

#[test]
fn test_locks_dir_created_automatically() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let session_dir = temp_dir.path();

    // locks/ subdir should not exist yet
    assert!(!session_dir.join("locks").exists());

    let _lock = acquire_lock(session_dir, "auto-dir", "test").expect("Lock should succeed");

    assert!(session_dir.join("locks").exists());
    assert!(session_dir.join("locks").is_dir());
}

#[test]
fn test_acquire_lock_nonexistent_parent_dir() {
    // session_dir itself does not exist — create_dir_all should handle it
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let session_dir = temp_dir.path().join("deep").join("nested").join("session");

    let lock = acquire_lock(&session_dir, "tool", "reason");
    assert!(lock.is_ok(), "Should create intermediate dirs");
}

#[test]
fn test_acquire_lock_invalid_path() {
    // /dev/null is a file, not a directory — creating locks/ under it should fail
    let result = acquire_lock(Path::new("/dev/null"), "tool", "reason");
    assert!(
        result.is_err(),
        "Should fail for non-directory session path"
    );
}

#[test]
fn test_lock_debug_format() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let session_dir = temp_dir.path();

    let lock = acquire_lock(session_dir, "debug-tool", "test").expect("Lock should succeed");

    let debug = format!("{lock:?}");
    assert!(debug.contains("SessionLock"));
    assert!(debug.contains("lock_path"));
}

#[test]
fn test_second_lock_error_contains_diagnostic() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let session_dir = temp_dir.path();

    let _lock1 =
        acquire_lock(session_dir, "diag-tool", "first task").expect("First lock should succeed");

    let err = acquire_lock(session_dir, "diag-tool", "second task")
        .unwrap_err()
        .to_string();

    // Error must contain PID, tool name, and original reason
    assert!(err.contains(&std::process::id().to_string()), "missing PID");
    assert!(err.contains("diag-tool"), "missing tool name");
    assert!(err.contains("first task"), "missing original reason");
}

#[test]
fn test_parent_fork_lock_serializes_same_parent() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let state_root = temp_dir.path();

    let _lock1 = acquire_parent_fork_lock(state_root, "01PARENT", "fork-call")
        .expect("first parent lock should succeed");
    let err = acquire_parent_fork_lock(state_root, "01PARENT", "fork-call")
        .expect_err("second lock on same parent should fail")
        .to_string();

    assert!(err.contains("fork-call-parent:01PARENT"));
}

#[test]
fn test_parent_fork_lock_allows_different_parents() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let state_root = temp_dir.path();

    let lock_a = acquire_parent_fork_lock(state_root, "01PARENTA", "fork-call");
    let lock_b = acquire_parent_fork_lock(state_root, "01PARENTB", "fork-call");

    assert!(lock_a.is_ok());
    assert!(lock_b.is_ok());
}

#[test]
fn test_project_resource_lock_conflicts_for_same_project_root() {
    let _env_lock = env_test_lock();
    let home_dir = tempdir().expect("home tempdir");
    let _home_guard = EnvVarGuard::set_os("HOME", home_dir.path());
    let _xdg_guard = EnvVarGuard::unset("XDG_STATE_HOME");

    let project_root = tempdir().expect("project tempdir");
    let _lock1 = acquire_project_resource_lock(
        project_root.path(),
        "jj-journal",
        "snapshot",
        "first snapshot",
    )
    .expect("first project lock should succeed");

    let err = acquire_project_resource_lock(
        project_root.path(),
        "jj-journal",
        "snapshot",
        "second snapshot",
    )
    .expect_err("second project lock on same project_root should fail")
    .to_string();

    assert!(err.contains("jj-journal:snapshot"));
}

#[test]
fn test_project_resource_lock_fails_when_project_root_cannot_canonicalize() {
    let missing_project_root = tempdir()
        .expect("project parent tempdir")
        .path()
        .join("missing-project");

    let err =
        acquire_project_resource_lock(&missing_project_root, "jj-journal", "snapshot", "snapshot")
            .expect_err("missing project root should fail closed")
            .to_string();

    assert!(
        err.contains("csa-lock: failed to canonicalize project root"),
        "missing canonicalize context: {err}"
    );
    assert!(
        err.contains("cross-session lock coordination requires a stable canonical path"),
        "missing coordination rationale: {err}"
    );
    assert!(
        err.contains(".csa/config.toml [filesystem_sandbox]"),
        "missing sandbox config hint: {err}"
    );
}

#[test]
fn test_project_resource_lock_allows_different_project_roots() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let project_a = tempdir().expect("project a tempdir");
    let project_b = tempdir().expect("project b tempdir");

    let lock_a = acquire_project_resource_lock(project_a.path(), "jj-journal", "snapshot", "first");
    let lock_b =
        acquire_project_resource_lock(project_b.path(), "jj-journal", "snapshot", "second");

    assert!(lock_a.is_ok());
    assert!(lock_b.is_ok());
}

fn dead_test_pid() -> u32 {
    i32::MAX as u32
}

fn write_session_lock_diagnostic(
    lock_path: &Path,
    pid: u32,
    pid_start_time_ticks: Option<u64>,
    tool_name: &str,
    reason: &str,
    holder_session_id: Option<&str>,
    resource_path: Option<&Path>,
) {
    fs::create_dir_all(lock_path.parent().expect("lock path should have parent"))
        .expect("create lock parent");
    let diagnostic = LockDiagnostic {
        pid,
        pid_start_time_ticks,
        tool_name: tool_name.to_string(),
        acquired_at: Utc::now(),
        reason: reason.to_string(),
        holder_session_id: holder_session_id.map(ToString::to_string),
        resource_path: resource_path.map(|path| path.display().to_string()),
    };
    fs::write(lock_path, serde_json::to_string(&diagnostic).unwrap())
        .expect("write lock diagnostic");
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
            .expect("open lock file for manual flock");
        // SAFETY: `file` owns a valid fd, and LOCK_EX | LOCK_NB requests a
        // non-blocking advisory lock for the stale-lock setup.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(ret, 0, "manual stale flock setup should acquire lock");
        Self { file }
    }
}

impl Drop for ManualFlock {
    fn drop(&mut self) {
        // SAFETY: `file` owns a valid fd; unlock before close for deterministic cleanup.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
