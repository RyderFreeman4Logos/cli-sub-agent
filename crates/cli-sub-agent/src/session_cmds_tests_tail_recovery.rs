use super::*;
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
fn synthesized_wait_next_step_returns_directive_for_unpushed_commit_recovery() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();
    let session = create_session(
        project,
        Some("wait-next-step-unpushed"),
        None,
        Some("codex"),
    )
    .unwrap();
    let session_dir = get_session_dir(project, &session.meta_session_id).unwrap();
    std::fs::create_dir_all(session_dir.join("output")).unwrap();

    std::fs::write(
        session_dir.join("output").join("unpushed_commits.json"),
        r#"{
  "branch": "fix/session-progress",
  "remote_ref": null,
  "commits_ahead": 2,
  "commits": [
    {"sha": "abc123", "subject": "feat: first progress"},
    {"sha": "def456", "subject": "fix: second progress"}
  ],
  "recovery_command": "git push -u origin fix/session-progress"
}"#,
    )
    .unwrap();

    let directive = synthesized_wait_next_step(&session_dir)
        .unwrap()
        .expect("directive should be synthesized");
    assert!(directive.contains("CSA:NEXT_STEP"));
    assert!(directive.contains("git push -u origin fix/session-progress"));
}
