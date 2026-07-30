#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::{Child, ExitStatus};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "linux", target_os = "macos"))]
const DAEMON_FIXTURE_TERM_GRACE: Duration = Duration::from_millis(100);
#[cfg(any(target_os = "linux", target_os = "macos"))]
const DAEMON_FIXTURE_REAP_TIMEOUT: Duration = Duration::from_secs(1);
#[cfg(any(target_os = "linux", target_os = "macos"))]
const DAEMON_FIXTURE_REAP_POLL: Duration = Duration::from_millis(10);

/// Owns a daemon-like fixture's session/process group until its leader is reaped.
///
/// The shell leader is created with `setsid`, so its PID is also the process-group
/// ID. Cleanup signals the group before reaping the leader; after reaping, a PID
/// can be reused and a negative-PGID signal would no longer be safe.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) struct DaemonLikeProcess {
    child: Option<Child>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl DaemonLikeProcess {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    pub(crate) fn id(&self) -> u32 {
        self.child
            .as_ref()
            .expect("daemon fixture leader must remain unreaped")
            .id()
    }

    /// Reap after an operation that is itself responsible for group termination.
    ///
    /// Session-kill fixture tests use this after exercising production's
    /// group-aware signal path. It deliberately does not issue a post-reap group
    /// signal because the leader PID may then be reused.
    pub(crate) fn wait_for_external_termination(&mut self) -> std::io::Result<ExitStatus> {
        self.child
            .take()
            .expect("daemon fixture leader must remain unreaped")
            .wait()
    }

    /// Terminate the entire fixture process group and reap its leader within a
    /// bounded interval. The final group signal occurs while the leader remains
    /// unreaped, so the negative PGID still belongs to this fixture.
    pub(crate) fn terminate_and_reap(&mut self) -> std::io::Result<ExitStatus> {
        let pid = self.id() as libc::pid_t;
        let pgid = -pid;

        // SAFETY: `pid` is an unreaped leader created by `setsid`; its process
        // group is therefore this fixture's group, not an unrelated group.
        let _ = unsafe { libc::kill(pgid, libc::SIGTERM) };
        std::thread::sleep(DAEMON_FIXTURE_TERM_GRACE);
        // SAFETY: the leader is still owned and unreaped, so its PID cannot be
        // reused before this final process-group signal.
        let kill_rc = unsafe { libc::kill(pgid, libc::SIGKILL) };
        if kill_rc != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
            // Fall back to the exact leader only if the group signal itself was
            // rejected for a reason other than the group already being empty.
            if let Some(child) = self.child.as_mut() {
                let _ = child.kill();
            }
        }

        let deadline = Instant::now() + DAEMON_FIXTURE_REAP_TIMEOUT;
        loop {
            let status = self
                .child
                .as_mut()
                .expect("daemon fixture leader must remain unreaped")
                .try_wait()?;
            if let Some(status) = status {
                self.child.take();
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timed out reaping daemon fixture leader after group termination",
                ));
            }
            std::thread::sleep(DAEMON_FIXTURE_REAP_POLL);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Drop for DaemonLikeProcess {
    fn drop(&mut self) {
        // Arms at spawn and remains armed through assertions/panics. Ignore the
        // bounded cleanup error here; an unreaped leader retains ownership, so
        // no unsafe post-reap group signal can occur.
        if self.child.is_some() {
            let _ = self.terminate_and_reap();
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn spawn_daemon_like_process(session_id: &str) -> DaemonLikeProcess {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    let mut cmd = Command::new("sh");
    // Keep the shell itself as the live session leader and keep the session id in
    // its command line. macOS legacy PID validation relies on `ps` command-line
    // context (there is no `/proc` start-time check), so a bare `sleep 60`
    // fixture can look like an unrelated process there.
    cmd.arg("-c").arg(format!(
        "while :; do sleep 60; done # csa-daemon {session_id}"
    ));
    // SAFETY: test fixture only; makes the child its own session leader like a daemon.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    DaemonLikeProcess::new(cmd.spawn().expect("spawn daemon-like child"))
}

#[cfg(target_os = "linux")]
pub(crate) fn attach_test_daemon_pid_record(pid: u32) -> String {
    format!("{pid}\n")
}

#[cfg(target_os = "macos")]
pub(crate) fn attach_test_daemon_pid_record(pid: u32) -> String {
    format!("{pid} 0\n")
}
