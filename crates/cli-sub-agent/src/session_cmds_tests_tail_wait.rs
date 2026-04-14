use super::*;
use crate::session_cmds_daemon::{WaitReconciliationOutcome, handle_session_wait_with_hooks};
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

#[cfg(unix)]
#[test]
fn handle_session_wait_retires_active_session_after_dead_failure_completion_packet() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-retire-dead-completion-packet"),
        None,
        Some("codex"),
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 1\nstatus = \"failure\"\n",
    )
    .unwrap();

    let exit_code = handle_session_wait(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        1,
    )
    .unwrap();

    assert_eq!(exit_code, 1);

    let result = load_result(project, &session_id)
        .unwrap()
        .expect("wait should synthesize a terminal result for a dead active session");
    assert_eq!(result.status, "failure");

    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(persisted.phase, SessionPhase::Retired);
    assert_eq!(
        persisted.termination_reason.as_deref(),
        Some("orphaned_process")
    );
}

#[test]
fn handle_session_wait_prefers_synthetic_failure_status_and_exit_code_over_completion_packet() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-synthetic-exit-code"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write completion packet");

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        1,
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: true,
                synthetic: true,
            })
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should succeed");

    assert_eq!(exit_code, 1);
    assert_eq!(
        emitted_completion,
        Some((session_id, "failure".to_string(), 1, true))
    );
}
