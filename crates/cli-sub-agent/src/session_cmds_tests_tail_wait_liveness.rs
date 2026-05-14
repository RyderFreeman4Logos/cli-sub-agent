use super::*;
use crate::session_cmds_daemon::{
    WaitBehavior, WaitLoopTiming, WaitReconciliationOutcome, handle_session_wait_with_hooks,
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

/// When PID-level detection misses (session_has_terminal_process=false) but broader
/// filesystem liveness signals remain (is_alive=true due to recent session writes),
/// the wait must continue polling and eventually return 124 (timeout) — not 1 (failure).
///
/// Regression test for #1396.
#[test]
fn handle_session_wait_continues_polling_when_pid_missing_but_liveness_signals_present() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    // Create session with no daemon.pid or lock files — session_has_terminal_process returns
    // false. The session directory itself was just written (state.toml), so
    // ToolLiveness::is_alive returns true via has_recent_session_write_signal.
    let session = create_session(
        project,
        Some("wait-pid-missing-liveness-present"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");

    // Verify preconditions: no PID signals, but recent filesystem writes make is_alive=true.
    assert!(
        !csa_process::ToolLiveness::has_live_process(&session_dir),
        "test requires session_has_terminal_process=false: no live process"
    );
    assert!(
        !csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir),
        "test requires session_has_terminal_process=false: no daemon pid"
    );
    assert!(
        csa_process::ToolLiveness::is_alive(&session_dir),
        "recent session writes must make is_alive return true"
    );

    let exit_code = handle_session_wait_with_hooks(
        session_id,
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(1),
                memory_sample_interval: std::time::Duration::from_secs(15),
            },
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |_sid, _status, _exit_code, _synthetic, _mirror_to_stdout| {},
    )
    .expect("wait should succeed");

    assert_eq!(
        exit_code, 124,
        "wait must time out (124) not report failure (1) when liveness signals exist"
    );
}
