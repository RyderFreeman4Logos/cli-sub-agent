//! Daemon stderr rotation: redirect fd 2 through a [`SpoolRotator`] to bound
//! `stderr.log` growth.
//!
//! In daemon mode the child process's stderr is a plain file opened by
//! [`daemon::spawn_daemon`].  All `eprint!`/`tracing` output, plus tee'd
//! stdout/stderr lines, write directly to that file with no size limit.
//!
//! This module replaces fd 2 with a pipe, then spawns a background thread that
//! reads from the pipe and writes through a [`SpoolRotator`] into `stderr.log`.
//! The rotation applies the same mechanism used for `output.log` spools.

use std::io::Read;
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::output_helpers::SpoolRotator;

/// Default max stderr spool size: 50 MiB.
///
/// stderr is typically more verbose than stdout (tracing output, tee'd lines),
/// so the default is larger than `DEFAULT_SPOOL_MAX_BYTES` (32 MiB).
pub const DEFAULT_STDERR_SPOOL_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// Default timeout for joining the stderr drain thread during shutdown (seconds).
///
/// If a child process inherits fd 2 and outlives the daemon, the drain thread's
/// `read(pipe)` blocks indefinitely.  This timeout prevents daemon shutdown from
/// hanging in that scenario.
pub const DEFAULT_DRAIN_TIMEOUT_SECS: u64 = 5;

/// Guard returned by [`install_stderr_rotation`].
///
/// When dropped, signals the background reader thread to stop and waits for it
/// to finish.  This ensures the final stderr.log rotation/flush occurs before
/// the daemon process exits.
///
/// Call [`finalize`](Self::finalize) explicitly before `process::exit()` since
/// `exit()` skips Drop.
pub struct StderrRotationGuard {
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The raw fd for the pipe write-end (i.e. fd 2 after dup2).  Stored so
    /// we can close it to unblock the reader thread during shutdown.
    write_fd: i32,
    /// Channel receiver for drain-thread completion signal.
    done_rx: Option<std::sync::mpsc::Receiver<()>>,
    /// Maximum time to wait for the drain thread to finish during shutdown.
    drain_timeout: Duration,
}

impl StderrRotationGuard {
    /// Explicitly shut down the stderr rotation, flush the spool, and join the
    /// background thread.  Call this before `process::exit()` which skips Drop.
    ///
    /// After this call the guard is inert — a subsequent Drop is a no-op.
    pub fn finalize(&mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        self.shutdown.store(true, Ordering::Release);

        // Close the write end of the pipe so the reader thread's `read()`
        // returns EOF and exits promptly, rather than blocking indefinitely.
        if self.write_fd >= 0 {
            // SAFETY: write_fd is the pipe write-end (fd 2) that we own.
            // Closing it unblocks the reader thread.  After this, any
            // further writes to fd 2 will get EBADF, which is acceptable
            // during shutdown.
            unsafe { libc::close(self.write_fd) };
            self.write_fd = -1;
        }

        if let Some(handle) = self.thread.take() {
            // Wait for the drain thread to signal completion, with a timeout
            // to prevent hanging if a child process inherited the pipe fd.
            let completed = self
                .done_rx
                .as_ref()
                .map(|rx| rx.recv_timeout(self.drain_timeout).is_ok())
                .unwrap_or(false);

            if completed {
                // Thread signaled done — join should return immediately.
                let _ = handle.join();
            } else {
                tracing::warn!(
                    timeout_secs = self.drain_timeout.as_secs(),
                    "stderr drain thread did not finish within timeout; abandoning thread"
                );
                // Intentionally leak the JoinHandle — OS cleans up on process exit.
            }
        }
    }
}

impl Drop for StderrRotationGuard {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

/// Replace the process's stderr (fd 2) with a pipe whose read end is drained
/// through a [`SpoolRotator`] writing to `stderr_path`.
///
/// Returns a guard that must be kept alive for the duration of the daemon child
/// process.  Dropping the guard flushes and finalizes the spool.
///
/// # Errors
///
/// Returns an error if the pipe or fd manipulation fails.  The caller should
/// treat this as non-fatal and fall back to unbounded stderr.
pub fn install_stderr_rotation(
    stderr_path: &Path,
    max_bytes: u64,
    keep_rotated: bool,
    drain_timeout: Duration,
) -> std::io::Result<StderrRotationGuard> {
    // Save the original fd 2 so we can restore it if thread spawn fails.
    // SAFETY: dup is POSIX, returns a new fd pointing to the same file.
    let saved_fd2 = unsafe { libc::dup(2) };
    if saved_fd2 == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // Mark saved fd as close-on-exec so it doesn't leak to children.
    // SAFETY: fcntl on a valid fd we own.
    unsafe { libc::fcntl(saved_fd2, libc::F_SETFD, libc::FD_CLOEXEC) };

    // Create an OS pipe.
    let (read_fd, write_fd) = match create_cloexec_pipe() {
        Ok(fds) => fds,
        Err(err) => {
            unsafe { libc::close(saved_fd2) };
            return Err(err);
        }
    };

    // Redirect fd 2 (stderr) to the pipe write-end.
    // SAFETY: dup2 is async-signal-safe and replaces fd 2 atomically.
    let ret = unsafe { libc::dup2(write_fd, 2) };
    if ret == -1 {
        let err = std::io::Error::last_os_error();
        // Clean up pipe fds and restore original fd 2.
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
            libc::dup2(saved_fd2, 2);
            libc::close(saved_fd2);
        }
        return Err(err);
    }

    // Close the original write_fd (fd 2 is now the write end).
    // SAFETY: write_fd is a valid fd we just created.
    if write_fd != 2 {
        unsafe { libc::close(write_fd) };
    }

    // Clear O_CLOEXEC on fd 2 so child processes inherit stderr normally.
    // SAFETY: fcntl on fd 2 is safe.
    unsafe {
        let flags = libc::fcntl(2, libc::F_GETFD);
        if flags != -1 {
            libc::fcntl(2, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
        }
    }

    // Wrap the read end in a File for the background thread.
    // SAFETY: read_fd is a valid fd we own.
    let read_file = unsafe { std::fs::File::from_raw_fd(read_fd) };

    let stderr_path = stderr_path.to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = std::sync::mpsc::channel();

    let thread = match std::thread::Builder::new()
        .name("csa-stderr-rotation".to_string())
        .spawn(move || {
            run_stderr_drain(read_file, &stderr_path, max_bytes, keep_rotated);
            let _ = done_tx.send(());
        }) {
        Ok(handle) => handle,
        Err(e) => {
            // Thread spawn failed: fd 2 points to pipe with no reader.
            // Restore original stderr so the process can still write errors.
            // SAFETY: saved_fd2 is a valid dup of the original fd 2.
            unsafe {
                libc::dup2(saved_fd2, 2);
                libc::close(saved_fd2);
            }
            // read_file was moved into the closure which was never spawned;
            // it will be dropped here, closing the pipe read-end.
            return Err(std::io::Error::other(e));
        }
    };

    // Thread spawned successfully; saved fd is no longer needed.
    // SAFETY: saved_fd2 is a valid fd we own.
    unsafe { libc::close(saved_fd2) };

    Ok(StderrRotationGuard {
        shutdown,
        thread: Some(thread),
        write_fd: 2,
        done_rx: Some(done_rx),
        drain_timeout,
    })
}

/// Background thread body: read from pipe, write through SpoolRotator.
fn run_stderr_drain(
    mut pipe: std::fs::File,
    stderr_path: &Path,
    max_bytes: u64,
    keep_rotated: bool,
) {
    let mut rotator = match SpoolRotator::open(stderr_path, max_bytes, keep_rotated) {
        Ok(r) => r,
        Err(e) => {
            // Cannot use eprintln! here (fd 2 is the pipe we're reading from).
            // Best effort: write to the spool path as a plain file.
            let _ = std::fs::write(
                stderr_path,
                format!("[csa-stderr-rotation] failed to open rotator: {e}\n"),
            );
            return;
        }
    };

    let mut buf = [0u8; 8192];
    loop {
        // NOTE: No shutdown atomic check here — we rely on EOF from the closed
        // write-end as the sole termination signal.  Checking `shutdown` before
        // `read()` caused a race where finalize() set the flag AND closed the
        // write-end, but the reader saw the flag first and exited *before*
        // draining remaining bytes from the pipe buffer.  See issue #571.
        match pipe.read(&mut buf) {
            Ok(0) => break, // EOF — write-end closed (process exiting)
            Ok(n) => {
                let _ = rotator.write(&buf[..n]);
                // Flush periodically to keep spool visible for `csa session logs`.
                let _ = rotator.flush();
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }

    // Finalize: flush and run sanitization plan.
    if let Ok(plan) = rotator.finalize() {
        let _ = crate::sanitize_spool_plan(plan, None);
    }
}

/// Query the raw fd for the stderr.log file size without going through
/// the rotator.  Useful for daemon health checks.
pub fn stderr_log_size(stderr_path: &Path) -> Option<u64> {
    std::fs::metadata(stderr_path).ok().map(|m| m.len())
}

fn create_cloexec_pipe() -> std::io::Result<(i32, i32)> {
    let mut fds = [0i32; 2];

    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // SAFETY: `fds` points to storage for two file descriptors and
        // `O_CLOEXEC` prevents descriptor leaks across exec.
        let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok((fds[0], fds[1]))
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        // SAFETY: `fds` points to storage for two file descriptors.
        let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
        for &fd in &fds {
            // SAFETY: each fd was just created by `pipe` and is owned here.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            if flags == -1 {
                let err = std::io::Error::last_os_error();
                unsafe {
                    libc::close(fds[0]);
                    libc::close(fds[1]);
                }
                return Err(err);
            }
            // SAFETY: each fd was just created by `pipe` and is owned here.
            let ret = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
            if ret == -1 {
                let err = std::io::Error::last_os_error();
                unsafe {
                    libc::close(fds[0]);
                    libc::close(fds[1]);
                }
                return Err(err);
            }
        }
        Ok((fds[0], fds[1]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::os::unix::io::FromRawFd;

    /// Create an OS pipe and return (read_file, write_file).
    ///
    /// Uses `O_CLOEXEC` to prevent fd leak.  Both ends are wrapped in
    /// `std::fs::File` so they are closed on drop.
    fn make_pipe() -> (std::fs::File, std::fs::File) {
        let (read_fd, write_fd) = create_cloexec_pipe().expect("create cloexec pipe");
        // SAFETY: fds are valid, we own them.
        unsafe {
            (
                std::fs::File::from_raw_fd(read_fd),
                std::fs::File::from_raw_fd(write_fd),
            )
        }
    }

    /// Test that `run_stderr_drain` reads from a pipe and writes data to the
    /// spool file via `SpoolRotator`.
    ///
    /// This test does NOT modify fd 2 — it creates an independent pipe pair and
    /// passes the read end directly to the drain function.
    #[test]
    fn test_stderr_drain_captures_written_data() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let stderr_path = tmp.path().join("stderr.log");

        let (read_file, mut write_file) = make_pipe();

        let path_clone = stderr_path.clone();
        let handle = std::thread::spawn(move || {
            run_stderr_drain(read_file, &path_clone, 1024 * 1024, true);
        });

        // Write data to the pipe write-end.
        write_file
            .write_all(b"hello from pipe\n")
            .expect("write to pipe");
        write_file
            .write_all(b"second line\n")
            .expect("write to pipe");

        // Close the write end to signal EOF to the reader thread.
        drop(write_file);

        // Wait for the drain thread to finish.
        handle.join().expect("drain thread panicked");

        let content = std::fs::read_to_string(&stderr_path).expect("read stderr.log");
        assert!(
            content.contains("hello from pipe"),
            "pipe data should appear in stderr.log, got: {content:?}"
        );
        assert!(
            content.contains("second line"),
            "second line should appear in stderr.log, got: {content:?}"
        );
    }

    /// Test that rotation triggers when written data exceeds `max_bytes`.
    ///
    /// The drain thread reads in 8 KiB chunks and calls `SpoolRotator::write`
    /// per chunk.  Rotation fires when `current_file_bytes > 0 && current +
    /// incoming > max`.  We need the drain thread to process at least two
    /// separate `read()` calls so the second one sees `current_file_bytes > 0`.
    /// We achieve this by writing in batches with a short sleep in between.
    #[test]
    fn test_stderr_rotation_triggers_on_overflow() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let stderr_path = tmp.path().join("stderr.log");
        let rotated_path = tmp.path().join("stderr.log.rotated");

        let (read_file, mut write_file) = make_pipe();

        let max_bytes = 256u64;
        let path_clone = stderr_path.clone();
        let handle = std::thread::spawn(move || {
            run_stderr_drain(read_file, &path_clone, max_bytes, true);
        });

        // Write in separate batches with pauses so the drain thread processes
        // them as separate read() calls, allowing rotation to trigger.
        let line = "X".repeat(100) + "\n";
        for _ in 0..5 {
            write_file
                .write_all(line.as_bytes())
                .expect("write to pipe");
            // Flush + sleep to let the drain thread consume this batch.
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        for _ in 0..5 {
            write_file
                .write_all(line.as_bytes())
                .expect("write to pipe");
            std::thread::sleep(std::time::Duration::from_millis(30));
        }

        // Close write end → EOF → drain thread exits.
        drop(write_file);
        handle.join().expect("drain thread panicked");

        assert!(
            rotated_path.exists(),
            "stderr.log.rotated should exist after rotation"
        );
        let current_size = std::fs::metadata(&stderr_path)
            .expect("stderr.log metadata")
            .len();
        assert!(
            current_size <= max_bytes + 200, // some slack for sentinel line
            "current stderr.log should be bounded, got {current_size} bytes"
        );

        let content = std::fs::read_to_string(&stderr_path).expect("read stderr.log");
        assert!(
            content.contains("[CSA:TRUNCATED"),
            "current file should contain truncation sentinel, got: {content:?}"
        );
    }

    /// Test that `keep_rotated=false` causes the `.rotated` file to be cleaned
    /// up during finalization.
    #[test]
    fn test_stderr_rotation_no_keep_rotated() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let stderr_path = tmp.path().join("stderr.log");
        let rotated_path = tmp.path().join("stderr.log.rotated");

        let (read_file, mut write_file) = make_pipe();

        let max_bytes = 128u64;
        let path_clone = stderr_path.clone();
        let handle = std::thread::spawn(move || {
            run_stderr_drain(read_file, &path_clone, max_bytes, false);
        });

        // Write in batches to ensure multiple read() calls in drain thread.
        let line = "Y".repeat(80) + "\n";
        for _ in 0..10 {
            write_file
                .write_all(line.as_bytes())
                .expect("write to pipe");
            std::thread::sleep(std::time::Duration::from_millis(30));
        }

        drop(write_file);
        handle.join().expect("drain thread panicked");

        // With keep_rotated=false, the .rotated file should be cleaned up
        // by sanitize_spool_plan during finalization.
        assert!(
            !rotated_path.exists(),
            "stderr.log.rotated should be removed when keep_rotated=false"
        );
    }

    /// Test that `shutdown_inner` completes within the drain timeout even when
    /// the pipe write-end is kept open (simulating a child process that inherited
    /// fd 2 and outlives the daemon).
    #[test]
    fn test_drain_timeout_when_pipe_held_open() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let stderr_path = tmp.path().join("stderr.log");

        let (read_file, write_file) = make_pipe();
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        let path_clone = stderr_path.clone();
        let handle = std::thread::spawn(move || {
            run_stderr_drain(read_file, &path_clone, 1024 * 1024, true);
            let _ = done_tx.send(());
        });

        // Deliberately keep write_file alive — the drain thread will block on
        // read() because the pipe never reaches EOF.
        let timeout = Duration::from_secs(2);
        let mut guard = StderrRotationGuard {
            shutdown: Arc::new(AtomicBool::new(false)),
            thread: Some(handle),
            write_fd: -1, // We don't own fd 2 in this test.
            done_rx: Some(done_rx),
            drain_timeout: timeout,
        };

        let start = std::time::Instant::now();
        guard.shutdown_inner();
        let elapsed = start.elapsed();

        // shutdown_inner should return after ~timeout, not hang indefinitely.
        assert!(
            elapsed < timeout + Duration::from_secs(2),
            "shutdown_inner should return within timeout + slack, took {elapsed:?}"
        );
        assert!(
            elapsed >= timeout - Duration::from_millis(100),
            "shutdown_inner should wait at least ~timeout before abandoning, took {elapsed:?}"
        );

        // Clean up: drop write_file to unblock the drain thread, then let it
        // exit naturally so we don't leak the thread for the test runner.
        drop(write_file);
    }

    /// Regression test for issue #571: pipe data must be fully drained even
    /// when `StderrRotationGuard::finalize()` is called before the reader
    /// thread has consumed all buffered data.
    ///
    /// Before the fix, the reader loop checked `shutdown.load()` *before*
    /// `pipe.read()`, so it could exit the loop with unread data in the pipe
    /// buffer.  After the fix, EOF from the closed write-end is the only
    /// termination condition, guaranteeing full drainage.
    #[test]
    fn test_drain_after_finalize() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let stderr_path = tmp.path().join("stderr.log");

        // We test via the public install API by creating a *separate* pipe
        // pair (not modifying fd 2 of this test process) and driving the
        // drain function directly.
        let (read_file, mut write_file) = make_pipe();

        let path_clone = stderr_path.clone();
        let handle = std::thread::spawn(move || {
            run_stderr_drain(read_file, &path_clone, 1024 * 1024, true);
        });

        // Write a moderate amount of data — enough to sit in the pipe buffer.
        let payload = "drain-test-line\n".repeat(200);
        write_file
            .write_all(payload.as_bytes())
            .expect("write to pipe");

        // Simulate what finalize() does: close the write-end while there may
        // still be unread data in the pipe buffer.  The reader should drain
        // everything before exiting.
        drop(write_file);

        handle.join().expect("drain thread panicked");

        let content = std::fs::read_to_string(&stderr_path).expect("read stderr.log");
        let line_count = content.lines().count();
        assert_eq!(
            line_count, 200,
            "all 200 lines must be drained to stderr.log after write-end close, got {line_count}"
        );
        assert!(
            content.contains("drain-test-line"),
            "stderr.log must contain the test payload, got: {content:?}"
        );
    }
}
