use super::super::*;
use super::wait_for_process_command_line_contains;
use crate::test_process_group::ProcessGroupFixture;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn daemon_pid_is_alive_accepts_legacy_pid_with_session_id_context() {
    use std::os::unix::fs::PermissionsExt;

    const SESSION_ID: &str = "01TESTSESSIONCONTEXT0000000001";

    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join(SESSION_ID);
    fs::create_dir_all(&session_dir).expect("create session dir");
    let daemon_fixture = session_dir.join("daemon-sleep");
    fs::write(&daemon_fixture, "#!/bin/sh\nsleep 60\n").expect("write daemon fixture");
    let mut perms = fs::metadata(&daemon_fixture)
        .expect("daemon fixture metadata")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&daemon_fixture, perms).expect("mark daemon fixture executable");
    let child = ProcessGroupFixture::spawn(std::process::Command::new(&daemon_fixture));
    let pid = child.id();
    let cmdline_ready = wait_for_process_command_line_contains(pid, SESSION_ID);
    fs::write(session_dir.join(DAEMON_PID_FILE), format!("{pid}\n")).expect("write daemon pid");
    let daemon_pid_alive = ToolLiveness::daemon_pid_is_alive(&session_dir);

    assert!(
        cmdline_ready,
        "spawned daemon command line should expose session context"
    );
    assert!(daemon_pid_alive, "legacy bare daemon.pid should stay alive");
}
