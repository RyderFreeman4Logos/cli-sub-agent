use super::*;
use crate::session_cmds_daemon::{
    WaitBehavior, WaitLoopTiming, WaitReconciliationOutcome, handle_session_wait_with_hooks,
    try_acquire_session_wait_lock,
};
use crate::test_env_lock::TEST_ENV_LOCK;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe {
            match self.original.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[test]
fn session_wait_lock_creates_dot_wait_lock_file_and_rejects_duplicates() {
    let td = tempdir().expect("tempdir");

    let _first_lock =
        try_acquire_session_wait_lock(td.path()).expect("first wait lock acquisition");
    assert!(
        td.path().join(".wait.lock").is_file(),
        "wait lock file should be created on first acquisition"
    );

    let second_lock = try_acquire_session_wait_lock(td.path())
        .expect("second wait lock attempt should not error");
    assert!(
        second_lock.is_none(),
        "second concurrent wait lock attempt should be rejected"
    );
}

#[test]
fn session_wait_lock_reuses_unheld_stale_lock_file() {
    let td = tempdir().expect("tempdir");
    std::fs::write(td.path().join(".wait.lock"), r#"{"pid":1}"#).expect("write stale lock file");

    let lock = try_acquire_session_wait_lock(td.path())
        .expect("wait lock acquisition should not error")
        .expect("unheld stale lock file should not block acquisition");

    assert!(
        td.path().join(".wait.lock").is_file(),
        "wait lock file should remain present after acquisition"
    );

    drop(lock);
}

#[test]
fn session_wait_lock_rejects_locked_stale_pid_without_replacing_inode() {
    let td = tempdir().expect("tempdir");
    let lock_path = td.path().join(".wait.lock");
    let stale_pid = exited_child_pid();
    let mut stale_file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .expect("open stale lock file");
    writeln!(stale_file, "{{\"pid\":{stale_pid}}}").expect("write stale wait pid");
    stale_file.flush().expect("flush stale wait pid");

    // SAFETY: `stale_file` owns a valid fd; the test intentionally simulates a
    // stale inherited flock whose diagnostic PID no longer exists.
    let rc = unsafe { libc::flock(stale_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(rc, 0, "test setup should acquire stale flock");
    let original_inode = std::fs::metadata(&lock_path)
        .expect("lock path metadata")
        .ino();

    let blocked = try_acquire_session_wait_lock(td.path())
        .expect("stale locked wait lock check should not error");
    assert!(
        blocked.is_none(),
        "locked wait file should reject duplicates even when diagnostic pid is stale"
    );
    let current_inode = std::fs::metadata(&lock_path)
        .expect("lock path metadata after blocked acquire")
        .ino();
    assert_eq!(
        original_inode, current_inode,
        "blocked acquisition must not replace the lock path"
    );

    // SAFETY: `stale_file` owns the fd holding the test lock.
    unsafe {
        libc::flock(stale_file.as_raw_fd(), libc::LOCK_UN);
    }
}

fn exited_child_pid() -> u32 {
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("exit 0")
        .spawn()
        .expect("spawn short-lived child");
    let pid = child.id();
    child.wait().expect("wait for short-lived child");
    pid
}

fn init_git_repo(path: &Path) {
    let output = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(path)
        .output()
        .expect("git init should run");
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn handle_session_wait_rejects_duplicate_wait_before_entering_loop() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(project, Some("wait-lock-duplicate"), None, Some("codex"))
        .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    let _wait_lock = try_acquire_session_wait_lock(&session_dir)
        .expect("pre-acquire wait lock")
        .expect("wait lock should be acquired");

    let mut reconcile_called = false;
    let mut emitted_completion = false;
    let exit_code = handle_session_wait_with_hooks(
        session_id,
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            reconcile_called = true;
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |_sid: &str, _status: &str, _exit_code, _synthetic, _mirror_to_stdout| {
            emitted_completion = true;
        },
    )
    .expect("duplicate wait should short-circuit with exit code");

    assert_eq!(exit_code, 1);
    assert!(
        !reconcile_called,
        "duplicate wait should reject before the wait loop/reconcile hook"
    );
    assert!(
        !emitted_completion,
        "duplicate wait should not emit a completion signal"
    );
}

#[test]
fn handle_session_wait_treats_live_worktree_lock_as_progress_during_stale_precheck() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let mut session = create_session(
        project,
        Some("wait-live-lock-not-stale"),
        None,
        Some("codex"),
    )
    .expect("create session");
    session.last_accessed = chrono::Utc::now() - chrono::Duration::hours(24);
    let session_id = session.meta_session_id.clone();
    save_session(&session).expect("save stale active session");
    let _worktree_lock = csa_lock::acquire_worktree_write_lock(
        project,
        &session_id,
        &[],
        |_| false,
        |_| false,
        |_| false,
    )
    .expect("worktree write lock should be held by session");

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 0,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("live worktree lock should keep stale precheck from reconciling")
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should not classify a live worktree lock as stale");

    assert_eq!(exit_code, 0);
    assert_eq!(
        emitted_completion, None,
        "live lock should produce a healthy wait cap, not a stale failure"
    );
}

#[test]
fn handle_session_wait_sees_git_toplevel_worktree_lock_from_subdirectory() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let repo_root = td.path().join("repo");
    let project = repo_root.join("nested");
    std::fs::create_dir_all(&project).expect("create nested project dir");
    init_git_repo(&repo_root);

    let mut session = create_session(
        &project,
        Some("wait-subdir-live-lock-not-stale"),
        None,
        Some("codex"),
    )
    .expect("create session");
    session.last_accessed = chrono::Utc::now() - chrono::Duration::hours(24);
    let session_id = session.meta_session_id.clone();
    save_session(&session).expect("save stale active session");
    let _worktree_lock = csa_lock::acquire_worktree_write_lock(
        &repo_root,
        &session_id,
        &[],
        |_| false,
        |_| false,
        |_| false,
    )
    .expect("writer lock should be held at git toplevel");
    assert!(
        csa_lock::worktree_write_lock_is_held_by_session(&repo_root, &session_id)
            .expect("probe repo-root worktree lock"),
        "test setup requires a live git-toplevel worktree lock"
    );
    assert!(
        !csa_lock::worktree_write_lock_is_held_by_session(&project, &session_id)
            .expect("probe nested worktree lock"),
        "direct nested-root probe must miss the git-toplevel lock to prove the root mismatch"
    );

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 0,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("live git-toplevel worktree lock should keep session nonterminal")
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should see the git-toplevel worktree lock from a subdirectory");

    assert_eq!(exit_code, 0);
    assert_eq!(
        emitted_completion, None,
        "subdirectory wait must return the healthy wait-cap path, not stale failure or completion"
    );
}

#[test]
fn handle_session_wait_defers_terminal_result_while_worktree_lock_is_live() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-defers-result-while-worktree-lock-live"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let terminal_result = SessionResult {
        summary: "provider usage limit".to_string(),
        ..make_result("failure", 1)
    };
    save_result(project, &session_id, &terminal_result).expect("save terminal result");
    let worktree_lock = csa_lock::acquire_worktree_write_lock(
        project,
        &session_id,
        &[],
        |_| false,
        |_| false,
        |_| false,
    )
    .expect("worktree write lock should be held by session");

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 0,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("live worktree lock should keep session nonterminal")
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should defer terminal result while lock is live");

    assert_eq!(exit_code, 0);
    assert_eq!(
        emitted_completion, None,
        "wait must not emit terminal failure while the session still holds the worktree lock"
    );

    drop(worktree_lock);
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 0,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("terminal result after lock release should not need reconciliation")
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should trust terminal result after lock release");

    assert_eq!(exit_code, 1);
    assert_eq!(
        emitted_completion,
        Some((session_id, "failure".to_string(), 1, false))
    );
}
