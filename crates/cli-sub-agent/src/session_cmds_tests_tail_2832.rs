use super::*;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::session_cmds_daemon::session_cmds_daemon_test_support::{
    spawn_daemon_like_process, DaemonLikeProcess,
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn wait_for_daemon_fixture_visibility(
    session_dir: &std::path::Path,
    fixture: &DaemonLikeProcess,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while std::time::Instant::now() < deadline {
        if csa_process::ToolLiveness::has_live_process(session_dir) {
            return;
        }
        assert!(
            fixture.id() > 1,
            "daemon fixture leader must remain owned while waiting for liveness"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("daemon fixture never became visible to ToolLiveness");
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn handle_session_wait_ignores_completion_packet_while_daemon_alive() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-live-completion-packet"),
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

    let mut child = spawn_daemon_like_process(&session_id);
    std::fs::write(session_dir.join("daemon.pid"), child.id().to_string()).unwrap();
    wait_for_daemon_fixture_visibility(&session_dir, &child);

    let exit_code =
        handle_session_wait(session_id, Some(project.to_string_lossy().into_owned()), 1).unwrap();

    // Daemon is still alive when the 1s wait cap fires; the completion packet
    // recorded the daemon process exit, not the session outcome, so the wait
    // must NOT surface it. Under #1439, alive-at-cap returns the KV-warm code
    // (0) rather than the legacy 124.
    assert_eq!(
        exit_code, 0,
        "wait should emit the KV-warm exit instead of reporting completion while the daemon is still alive"
    );

    child
        .terminate_and_reap()
        .expect("terminate and reap daemon fixture group");
}
