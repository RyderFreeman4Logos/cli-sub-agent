#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn spawn_daemon_like_process(session_id: &str) -> std::process::Child {
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
    cmd.spawn().expect("spawn daemon-like child")
}

#[cfg(target_os = "linux")]
pub(crate) fn attach_test_daemon_pid_record(pid: u32) -> String {
    format!("{pid}\n")
}

#[cfg(target_os = "macos")]
pub(crate) fn attach_test_daemon_pid_record(pid: u32) -> String {
    format!("{pid} 0\n")
}
