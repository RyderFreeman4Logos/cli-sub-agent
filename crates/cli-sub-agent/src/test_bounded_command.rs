//! Bounded subprocess helpers for unit/integration tests (Rust 015).

#[cfg(test)]
use std::io::{Read, Seek, SeekFrom};
#[cfg(test)]
use std::process::{Command, ExitStatus, Output, Stdio};
#[cfg(test)]
use std::time::{Duration, Instant};

/// Run `command` with a hard wall-clock bound.
///
/// The child is placed in its own process group. On timeout the whole group is
/// signaled (TERM then KILL), and the direct child is synchronously reaped.
/// Output is captured in anonymous temporary files rather than pipes so a child
/// producing more than the pipe capacity cannot deadlock before it exits.
#[cfg(test)]
pub(crate) fn output_with_timeout(mut command: Command, timeout: Duration) -> Output {
    use std::os::unix::process::CommandExt;

    let mut stdout_file = tempfile::tempfile().expect("create bounded-command stdout file");
    let mut stderr_file = tempfile::tempfile().expect("create bounded-command stderr file");
    command
        .stdout(Stdio::from(
            stdout_file
                .try_clone()
                .expect("clone bounded-command stdout file"),
        ))
        .stderr(Stdio::from(
            stderr_file
                .try_clone()
                .expect("clone bounded-command stderr file"),
        ));
    // SAFETY: only setpgid(0, 0) in the child before exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = command.spawn().expect("spawn bounded test command");
    let pid = child.id() as i32;
    let deadline = Instant::now() + timeout;
    let mut last_status: Option<ExitStatus> = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                last_status = Some(status);
                break;
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => break,
            Err(error) => panic!("bounded test command wait failed: {error}"),
        }
    }

    if last_status.is_none() {
        terminate_process_group(pid);
        // Reap the direct child unconditionally. Temporary-file capture means
        // escaped descendants cannot keep this waiter blocked on inherited pipes.
        let _ = child.wait();
        panic!("bounded test command exceeded {timeout:?}");
    }

    let status = last_status.unwrap();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    stdout_file
        .seek(SeekFrom::Start(0))
        .expect("rewind bounded-command stdout file");
    stderr_file
        .seek(SeekFrom::Start(0))
        .expect("rewind bounded-command stderr file");
    stdout_file
        .read_to_end(&mut stdout)
        .expect("read bounded-command stdout file");
    stderr_file
        .read_to_end(&mut stderr)
        .expect("read bounded-command stderr file");
    Output {
        status,
        stdout,
        stderr,
    }
}

#[cfg(test)]
pub(crate) fn status_with_timeout(command: Command, timeout: Duration) -> std::process::ExitStatus {
    output_with_timeout(command, timeout).status
}

#[cfg(test)]
fn terminate_process_group(pid: i32) {
    // Negative PID targets the process group created via setpgid(0, 0).
    // SAFETY: pid is the positive child id we just spawned into its own group.
    unsafe {
        let _ = libc::kill(-pid, libc::SIGTERM);
    }
    std::thread::sleep(Duration::from_millis(50));
    unsafe {
        let _ = libc::kill(-pid, libc::SIGKILL);
    }
}
