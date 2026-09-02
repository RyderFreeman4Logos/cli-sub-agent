//! Bounded subprocess execution for capability probes and tests.

use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::io::AsRawFd;

/// Maximum combined stdout and stderr retained or accepted from one command.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Run a command with a wall-clock deadline and bounded output.
///
/// The command runs in its own process group. Every exit, timeout, or output
/// overflow terminates that group, then joins drainers that honor the deadline.
/// Production probes should pass [`MAX_OUTPUT_BYTES`].
pub fn output_with_timeout(
    mut command: Command,
    timeout: Duration,
    max_output_bytes: usize,
) -> io::Result<Output> {
    configure_process_group(&mut command);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("bounded command stdout pipe unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("bounded command stderr pipe unavailable"))?;
    let total = Arc::new(AtomicUsize::new(0));
    let output_exceeded = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + timeout;
    let stdout_reader = match spawn_reader(
        stdout,
        Arc::clone(&total),
        Arc::clone(&output_exceeded),
        max_output_bytes,
        deadline,
    ) {
        Ok(reader) => reader,
        Err(error) => {
            terminate_process_group(&mut child);
            let _ = child.wait();
            return Err(error);
        }
    };
    let stderr_reader = match spawn_reader(
        stderr,
        total,
        Arc::clone(&output_exceeded),
        max_output_bytes,
        deadline,
    ) {
        Ok(reader) => reader,
        Err(error) => {
            terminate_process_group(&mut child);
            let _ = child.wait();
            let _ = stdout_reader.join();
            return Err(error);
        }
    };

    loop {
        if output_exceeded.load(Ordering::Acquire) {
            terminate_process_group(&mut child);
            let status = child.wait();
            let _ = join_readers(stdout_reader, stderr_reader);
            status?;
            return Err(output_limit_error(max_output_bytes));
        }

        match child_exited_without_reaping(&mut child) {
            Ok(true) => {
                terminate_process_group(&mut child);
                let status = child.wait();
                let (stdout, stderr) = join_readers(stdout_reader, stderr_reader)?;
                if output_exceeded.load(Ordering::Acquire) {
                    return Err(output_limit_error(max_output_bytes));
                }
                return Ok(Output {
                    status: status?,
                    stdout,
                    stderr,
                });
            }
            Ok(false) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(false) => {
                terminate_process_group(&mut child);
                let status = child.wait();
                let _ = join_readers(stdout_reader, stderr_reader);
                status?;
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("bounded command exceeded {timeout:?}"),
                ));
            }
            Err(error) => {
                terminate_process_group(&mut child);
                let _ = child.wait();
                let _ = join_readers(stdout_reader, stderr_reader);
                return Err(error);
            }
        }
    }
}

/// Run a command with the same bounds when only its exit status is needed.
pub fn status_with_timeout(command: Command, timeout: Duration) -> io::Result<ExitStatus> {
    output_with_timeout(command, timeout, MAX_OUTPUT_BYTES).map(|output| output.status)
}

#[cfg(unix)]
fn spawn_reader<R: Read + AsRawFd + Send + 'static>(
    mut reader: R,
    total: Arc<AtomicUsize>,
    output_exceeded: Arc<AtomicBool>,
    max_output_bytes: usize,
    deadline: Instant,
) -> io::Result<JoinHandle<io::Result<Vec<u8>>>> {
    thread::Builder::new().spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            if !poll_readable(reader.as_raw_fd(), deadline)? {
                return Ok(output);
            }
            let read = match reader.read(&mut buffer) {
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if read == 0 {
                return Ok(output);
            }
            let start = total.fetch_add(read, Ordering::Relaxed);
            let remaining = max_output_bytes.saturating_sub(start);
            output.extend_from_slice(&buffer[..read.min(remaining)]);
            if read > remaining {
                output_exceeded.store(true, Ordering::Release);
            }
        }
    })
}

#[cfg(not(unix))]
fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    total: Arc<AtomicUsize>,
    output_exceeded: Arc<AtomicBool>,
    max_output_bytes: usize,
    deadline: Instant,
) -> io::Result<JoinHandle<io::Result<Vec<u8>>>> {
    thread::Builder::new().spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            if Instant::now() >= deadline {
                return Ok(output);
            }
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                return Ok(output);
            }
            let start = total.fetch_add(read, Ordering::Relaxed);
            let remaining = max_output_bytes.saturating_sub(start);
            output.extend_from_slice(&buffer[..read.min(remaining)]);
            if read > remaining {
                output_exceeded.store(true, Ordering::Release);
            }
        }
    })
}

#[cfg(unix)]
fn poll_readable(fd: std::os::unix::io::RawFd, deadline: Instant) -> io::Result<bool> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        let remaining = deadline.saturating_duration_since(now);
        let timeout_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        let mut fds = [libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        }];
        // SAFETY: fd is this command's owned stdout/stderr pipe; pollfd is stack-local.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout_ms) };
        if n < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        return Ok(n > 0);
    }
}

fn join_readers(
    stdout: JoinHandle<io::Result<Vec<u8>>>,
    stderr: JoinHandle<io::Result<Vec<u8>>>,
) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let stdout = join_reader(stdout)?;
    let stderr = join_reader(stderr)?;
    Ok((stdout, stderr))
}

fn join_reader(reader: JoinHandle<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| io::Error::other("bounded command reader panicked"))?
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: setpgid(0, 0) runs in the child before exec and only creates its
    // own process group for bounded cleanup.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn waitid_direct_child_without_reaping(pid: libc::pid_t) -> io::Result<libc::siginfo_t> {
    loop {
        // SAFETY: zero is the documented no-state-change sentinel for waitid.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: pid is our spawned child; WNOWAIT leaves it unreaped so the
        // process-group identity stays reserved until we terminate then reap.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if rc < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        return Ok(info);
    }
}

fn child_exited_without_reaping(child: &mut Child) -> io::Result<bool> {
    #[cfg(unix)]
    {
        let info = waitid_direct_child_without_reaping(child.id() as libc::pid_t)?;
        // SAFETY: waitid initialized info; si_pid == 0 means no state change.
        Ok(unsafe { info.si_pid() } != 0)
    }
    #[cfg(not(unix))]
    {
        Ok(child.try_wait()?.is_some())
    }
}

#[cfg(all(unix, test))]
thread_local! {
    static GROUP_TERMINATED_WHILE_UNREAPED: AtomicBool = const { AtomicBool::new(false) };
}

#[cfg(all(unix, test))]
fn direct_child_is_unreaped(pid: libc::pid_t) -> bool {
    waitid_direct_child_without_reaping(pid).is_ok()
}

fn terminate_process_group(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        #[cfg(test)]
        {
            GROUP_TERMINATED_WHILE_UNREAPED.with(|flag| {
                flag.store(direct_child_is_unreaped(pid), Ordering::Relaxed);
            });
        }
        // SAFETY: pid is the direct child placed in its own process group.
        unsafe {
            let _ = libc::kill(-pid, libc::SIGTERM);
        }
        thread::sleep(Duration::from_millis(50));
        // SAFETY: the same process-group ownership remains valid until reap.
        unsafe {
            let _ = libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

fn output_limit_error(max_output_bytes: usize) -> io::Error {
    io::Error::other(format!(
        "bounded command output limit of {max_output_bytes} bytes exceeded"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[cfg(unix)]
    fn reset_group_terminated_while_unreaped() {
        GROUP_TERMINATED_WHILE_UNREAPED.with(|flag| flag.store(false, Ordering::Relaxed));
    }

    #[cfg(unix)]
    fn assert_group_terminated_while_unreaped() {
        assert!(
            GROUP_TERMINATED_WHILE_UNREAPED.with(|flag| flag.load(Ordering::Relaxed)),
            "process-group termination must run while the direct child is still unreaped"
        );
    }

    #[cfg(unix)]
    fn child_without_descendants_command() -> Command {
        let mut command = Command::new("/bin/true");
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null());
        command
    }

    #[cfg(unix)]
    fn pipe_holding_descendant_command() -> Command {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("sleep 30 &\nexit 0")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null());
        command
    }

    #[cfg(unix)]
    fn assert_returns_within_deadline<T>(
        timeout: Duration,
        result: io::Result<T>,
        started: Instant,
    ) -> T {
        let elapsed = started.elapsed();
        assert!(
            elapsed <= timeout + Duration::from_secs(1),
            "bounded command must return within the wall-clock deadline; elapsed={elapsed:?}"
        );
        result.expect("direct child exited 0; helper must return after process-group cleanup")
    }

    #[cfg(unix)]
    #[test]
    fn output_with_timeout_terminates_group_while_child_without_descendants_is_unreaped() {
        reset_group_terminated_while_unreaped();
        let timeout = Duration::from_secs(5);
        let started = Instant::now();
        let output = assert_returns_within_deadline(
            timeout,
            output_with_timeout(
                child_without_descendants_command(),
                timeout,
                MAX_OUTPUT_BYTES,
            ),
            started,
        );
        assert!(output.status.success());
        assert_group_terminated_while_unreaped();
    }

    #[cfg(unix)]
    #[test]
    fn output_with_timeout_pipe_holding_descendant_returns_within_deadline() {
        reset_group_terminated_while_unreaped();
        let timeout = Duration::from_secs(5);
        let started = Instant::now();
        let output = assert_returns_within_deadline(
            timeout,
            output_with_timeout(pipe_holding_descendant_command(), timeout, MAX_OUTPUT_BYTES),
            started,
        );
        assert!(output.status.success());
        assert_group_terminated_while_unreaped();
    }

    #[cfg(unix)]
    #[test]
    fn status_with_timeout_pipe_holding_descendant_returns_within_deadline() {
        reset_group_terminated_while_unreaped();
        let timeout = Duration::from_secs(5);
        let started = Instant::now();
        let status = assert_returns_within_deadline(
            timeout,
            status_with_timeout(pipe_holding_descendant_command(), timeout),
            started,
        );
        assert!(status.success());
        assert_group_terminated_while_unreaped();
    }
}
