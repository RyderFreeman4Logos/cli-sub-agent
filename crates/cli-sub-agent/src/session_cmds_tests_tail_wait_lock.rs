use super::*;
use crate::session_cmds_daemon::{
    WaitBehavior, WaitLoopTiming, WaitReconciliationOutcome, handle_session_wait_with_hooks,
    try_acquire_session_wait_lock,
};
use crate::test_env_lock::TEST_ENV_LOCK;
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
