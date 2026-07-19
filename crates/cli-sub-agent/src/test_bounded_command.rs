//! Bounded subprocess helpers for unit/integration tests (Rust 015).

#[cfg(test)]
use std::process::{Command, Output, Stdio};
#[cfg(test)]
use std::time::Duration;

/// Run `command` with a hard wall-clock bound.
///
/// The child is placed in its own process group. On timeout the whole group is
/// signaled (TERM then KILL), pipes are drained via `wait_with_output`, and the
/// direct child is reaped before panicking.
#[cfg(test)]
pub(crate) fn output_with_timeout(mut command: Command, timeout: Duration) -> Output {
    use std::os::unix::process::CommandExt;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    // SAFETY: only setpgid(0, 0) in the child before exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = command.spawn().expect("spawn bounded test command");
    let pid = child.id() as i32;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => panic!("bounded test command failed to wait: {error}"),
        Err(_) => {
            terminate_process_group(pid);
            // Best-effort reap: waiter thread still owns Child and will collect.
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(Ok(_)) | Ok(Err(_)) => break,
                    Err(_) if Instant::now() < deadline => continue,
                    Err(_) => break,
                }
            }
            panic!("bounded test command exceeded {timeout:?}");
        }
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
