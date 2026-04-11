use super::*;

#[cfg(target_os = "linux")]
use crate::test_env_lock::TEST_ENV_LOCK;

#[cfg(target_os = "linux")]
struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

#[cfg(target_os = "linux")]
impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

#[cfg(target_os = "linux")]
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
fn attach_primary_output_prefers_output_log_for_acp_tools() {
    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "codex".to_string(),
        tool_locked: true,
    };
    let metadata_toml = toml::to_string_pretty(&metadata).expect("metadata toml");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        metadata_toml,
    )
    .expect("write metadata");

    assert_eq!(
        attach_primary_output_for_session(td.path()),
        AttachPrimaryOutput::OutputLog
    );
}

#[test]
fn attach_primary_output_keeps_stdout_for_legacy_tools() {
    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "opencode".to_string(),
        tool_locked: true,
    };
    let metadata_toml = toml::to_string_pretty(&metadata).expect("metadata toml");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        metadata_toml,
    )
    .expect("write metadata");

    assert_eq!(
        attach_primary_output_for_session(td.path()),
        AttachPrimaryOutput::StdoutLog
    );
}

#[cfg(target_os = "linux")]
#[test]
fn handle_session_attach_treats_stale_daemon_pid_as_dead() {
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;

    let td = tempfile::tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = csa_session::create_session(
        project,
        Some("attach-stale-daemon-pid"),
        None,
        Some("opencode"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = csa_session::get_session_dir(project, &session_id).expect("session dir");
    std::fs::write(session_dir.join("stdout.log"), "").expect("write stdout log");

    let mut child = Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("spawn child");
    std::fs::write(
        session_dir.join("daemon.pid"),
        format!("{} 0\n", child.id()),
    )
    .expect("write daemon pid");

    let (tx, rx) = mpsc::channel();
    let attach_session = session_id.clone();
    let attach_project = project.to_string_lossy().into_owned();
    let handle = std::thread::spawn(move || {
        let result = handle_session_attach(attach_session, false, Some(attach_project))
            .map_err(|err| err.to_string());
        let _ = tx.send(result);
    });

    let attach_result = rx.recv_timeout(Duration::from_secs(2));

    child.kill().ok();
    child.wait().ok();
    handle.join().expect("attach thread join");

    let exit_code = attach_result
        .expect("attach should converge instead of waiting on a reused PID")
        .expect("attach result");
    assert_eq!(exit_code, 1);
    let result = csa_session::load_result(project, &session_id)
        .expect("load result")
        .expect("synthetic result");
    assert_eq!(result.status, "failure");
}
