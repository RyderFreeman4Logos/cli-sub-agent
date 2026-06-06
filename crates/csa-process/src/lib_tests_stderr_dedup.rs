use super::*;
use std::ffi::OsString;
use std::io::Write as _;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

const DAEMON_SESSION_ID_ENV: &str = "CSA_DAEMON_SESSION_ID";
const DAEMON_SESSION_DIR_ENV: &str = "CSA_DAEMON_SESSION_DIR";
static DAEMON_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct ScopedDaemonEnv {
    original_id: Option<OsString>,
    original_dir: Option<OsString>,
}

impl ScopedDaemonEnv {
    fn set(session_dir: &Path) -> Self {
        let original = Self::capture();
        // SAFETY: tests that mutate daemon session env hold DAEMON_ENV_LOCK.
        unsafe {
            std::env::set_var(DAEMON_SESSION_ID_ENV, "01KTESTSESSION");
            std::env::set_var(DAEMON_SESSION_DIR_ENV, session_dir.as_os_str());
        }
        original
    }

    fn unset() -> Self {
        let original = Self::capture();
        // SAFETY: tests that mutate daemon session env hold DAEMON_ENV_LOCK.
        unsafe {
            std::env::remove_var(DAEMON_SESSION_ID_ENV);
            std::env::remove_var(DAEMON_SESSION_DIR_ENV);
        }
        original
    }

    fn capture() -> Self {
        Self {
            original_id: std::env::var_os(DAEMON_SESSION_ID_ENV),
            original_dir: std::env::var_os(DAEMON_SESSION_DIR_ENV),
        }
    }
}

impl Drop for ScopedDaemonEnv {
    fn drop(&mut self) {
        restore_env(DAEMON_SESSION_ID_ENV, self.original_id.take());
        restore_env(DAEMON_SESSION_DIR_ENV, self.original_dir.take());
    }
}

fn restore_env(key: &str, value: Option<OsString>) {
    // SAFETY: tests that mutate daemon session env hold DAEMON_ENV_LOCK.
    unsafe {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}

fn line_occurrences(content: &str, expected: &str) -> usize {
    content.lines().filter(|line| *line == expected).count()
}

fn emit_child_stderr(
    payload: &str,
    session_dir: &Path,
    stderr_spool: &mut Option<output_helpers::SpoolRotator>,
    parent_stderr: &mut std::fs::File,
) -> bool {
    let tee_to_parent = output_helpers::should_tee_stderr_to_parent(
        StreamMode::TeeToStderr,
        Some(session_dir),
        stderr_spool.is_some(),
    );
    output_helpers::spool_chunk(stderr_spool, payload.as_bytes());
    if tee_to_parent {
        parent_stderr
            .write_all(payload.as_bytes())
            .expect("write parent stderr");
        parent_stderr.flush().expect("flush parent stderr");
    }
    tee_to_parent
}

#[test]
fn daemon_stderr_same_target_spool_writes_child_line_once() {
    let _env_lock = DAEMON_ENV_LOCK.lock().expect("daemon env lock poisoned");
    let tmp = tempfile::tempdir().expect("tempdir");
    let _daemon_env = ScopedDaemonEnv::set(tmp.path());
    let stderr_log = tmp.path().join("stderr.log");
    let mut stderr_spool = Some(
        output_helpers::SpoolRotator::open(&stderr_log, DEFAULT_SPOOL_MAX_BYTES, true)
            .expect("open stderr spool"),
    );
    let mut same_target = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_log)
        .expect("open same tee target");

    let tee_to_parent = emit_child_stderr(
        "child-stderr-once\n",
        tmp.path(),
        &mut stderr_spool,
        &mut same_target,
    );
    assert!(!tee_to_parent);
    drop(same_target);
    stderr_spool
        .take()
        .expect("stderr spool")
        .finalize()
        .expect("finalize stderr spool");

    let content = std::fs::read_to_string(&stderr_log).expect("read stderr.log");
    assert_eq!(line_occurrences(&content, "child-stderr-once"), 1);
}

#[test]
fn foreground_stderr_distinct_targets_keep_spool_and_tee() {
    let _env_lock = DAEMON_ENV_LOCK.lock().expect("daemon env lock poisoned");
    let _daemon_env = ScopedDaemonEnv::unset();
    let tmp = tempfile::tempdir().expect("tempdir");
    let stderr_log = tmp.path().join("stderr.log");
    let tee_log = tmp.path().join("parent-stderr.log");
    let mut stderr_spool = Some(
        output_helpers::SpoolRotator::open(&stderr_log, DEFAULT_SPOOL_MAX_BYTES, true)
            .expect("open stderr spool"),
    );
    let mut tee_target = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&tee_log)
        .expect("open tee target");

    let tee_to_parent = emit_child_stderr(
        "foreground-child-stderr\n",
        tmp.path(),
        &mut stderr_spool,
        &mut tee_target,
    );
    assert!(tee_to_parent);
    stderr_spool
        .take()
        .expect("stderr spool")
        .finalize()
        .expect("finalize stderr spool");

    let spool_content = std::fs::read_to_string(&stderr_log).expect("read stderr.log");
    let tee_content = std::fs::read_to_string(&tee_log).expect("read tee log");
    assert_eq!(
        line_occurrences(&spool_content, "foreground-child-stderr"),
        1
    );
    assert_eq!(line_occurrences(&tee_content, "foreground-child-stderr"), 1);
}

#[test]
fn daemon_without_stderr_spool_keeps_parent_tee() {
    let _env_lock = DAEMON_ENV_LOCK.lock().expect("daemon env lock poisoned");
    let tmp = tempfile::tempdir().expect("tempdir");
    let other = tempfile::tempdir().expect("other tempdir");
    let _daemon_env = ScopedDaemonEnv::set(tmp.path());

    assert!(output_helpers::should_tee_stderr_to_parent(
        StreamMode::TeeToStderr,
        Some(tmp.path()),
        false
    ));
    assert!(output_helpers::should_tee_stderr_to_parent(
        StreamMode::TeeToStderr,
        Some(other.path()),
        true
    ));
    assert!(!output_helpers::should_tee_stderr_to_parent(
        StreamMode::BufferOnly,
        Some(tmp.path()),
        false
    ));
}
