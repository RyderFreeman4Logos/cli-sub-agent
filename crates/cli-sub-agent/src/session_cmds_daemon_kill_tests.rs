#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::session_cmds_daemon_test_support::spawn_daemon_like_process;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::*;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn handle_session_kill_accepts_legacy_stderr_pid() {
    let td = tempfile::tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = ScopedEnvVarRestore::set("HOME", td.path());
    let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session =
        csa_session::create_session(project, Some("kill-legacy-stderr"), None, Some("opencode"))
            .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = csa_session::get_session_dir(project, &session_id).expect("session dir");

    let mut child = spawn_daemon_like_process(&session_id);
    let child_pid = child.id();
    std::fs::write(
        session_dir.join("stderr.log"),
        format!(
            "<!-- CSA:SESSION_STARTED id={} pid={} dir=\"{}\" wait_cmd=\"\" attach_cmd=\"\" -->\n",
            session_id,
            child_pid,
            session_dir.display()
        ),
    )
    .expect("write legacy stderr pid");

    let daemon_visible = (0..20).any(|_| {
        if csa_process::ToolLiveness::daemon_pid_for_signal(&session_dir) == Some(child_pid) {
            true
        } else {
            std::thread::sleep(std::time::Duration::from_millis(25));
            false
        }
    });
    assert!(
        daemon_visible,
        "legacy stderr PID fixture must be recognized as a live session process before kill"
    );

    let reaper = std::thread::spawn(move || child.wait().expect("wait child"));

    handle_session_kill(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
    )
    .expect("legacy kill should succeed");

    let status = reaper.join().expect("reaper join");
    assert!(
        !status.success(),
        "legacy daemon process should be terminated by session kill"
    );
}
