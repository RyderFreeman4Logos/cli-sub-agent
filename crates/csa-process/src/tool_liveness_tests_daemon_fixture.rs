use super::super::*;
use super::wait_for_process_command_line_contains;

/// Own a daemon-style test fixture and clean up every process in its group.
///
/// The fixture command is its own session leader, so a shell script that starts
/// a child cannot leave that child holding inherited test-harness file
/// descriptors after the script leader is terminated.
#[cfg(any(target_os = "linux", target_os = "macos"))]
struct DaemonFixtureProcess {
    child: Option<std::process::Child>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl DaemonFixtureProcess {
    fn spawn(program: &std::path::Path) -> Self {
        use std::os::unix::process::CommandExt;

        let mut command = std::process::Command::new(program);
        // SAFETY: test fixture only; gives the fixture a private process group.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Self {
            child: Some(command.spawn().expect("spawn daemon fixture")),
        }
    }

    fn id(&self) -> u32 {
        self.child
            .as_ref()
            .expect("daemon fixture leader must remain unreaped")
            .id()
    }

    fn terminate_and_reap(&mut self) {
        let pid = self.id() as libc::pid_t;
        // SAFETY: the unreaped setsid leader owns this fixture's process group.
        let _ = unsafe { libc::kill(-pid, libc::SIGTERM) };
        std::thread::sleep(std::time::Duration::from_millis(100));
        // SAFETY: the leader remains unreaped, so its PID cannot have been reused.
        let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
        let _ = self
            .child
            .take()
            .expect("daemon fixture leader must remain unreaped")
            .wait();
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Drop for DaemonFixtureProcess {
    fn drop(&mut self) {
        if self.child.is_some() {
            self.terminate_and_reap();
        }
    }
}

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
    let child = DaemonFixtureProcess::spawn(&daemon_fixture);
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
