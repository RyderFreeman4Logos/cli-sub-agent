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

use super::output_helpers::SpoolRotator;

/// Default max stderr spool size: 50 MiB.
///
/// stderr is typically more verbose than stdout (tracing output, tee'd lines),
/// so the default is larger than `DEFAULT_SPOOL_MAX_BYTES` (32 MiB).
pub const DEFAULT_STDERR_SPOOL_MAX_BYTES: u64 = 50 * 1024 * 1024;

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
            let _ = handle.join();
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
) -> std::io::Result<StderrRotationGuard> {
    // Create an OS pipe.
    let (read_fd, write_fd) = {
        let mut fds = [0i32; 2];
        // SAFETY: pipe2 is POSIX, O_CLOEXEC prevents fd leak to children.
        let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
        (fds[0], fds[1])
    };

    // Redirect fd 2 (stderr) to the pipe write-end.
    // SAFETY: dup2 is async-signal-safe and replaces fd 2 atomically.
    let ret = unsafe { libc::dup2(write_fd, 2) };
    if ret == -1 {
        // Clean up pipe fds on failure.
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return Err(std::io::Error::last_os_error());
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
    let shutdown_clone = shutdown.clone();

    let thread = std::thread::Builder::new()
        .name("csa-stderr-rotation".to_string())
        .spawn(move || {
            run_stderr_drain(
                read_file,
                &stderr_path,
                max_bytes,
                keep_rotated,
                shutdown_clone,
            );
        })
        .map_err(std::io::Error::other)?;

    Ok(StderrRotationGuard {
        shutdown,
        thread: Some(thread),
        write_fd: 2,
    })
}

/// Background thread body: read from pipe, write through SpoolRotator.
fn run_stderr_drain(
    mut pipe: std::fs::File,
    stderr_path: &Path,
    max_bytes: u64,
    keep_rotated: bool,
    shutdown: Arc<AtomicBool>,
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
        if shutdown.load(Ordering::Acquire) {
            break;
        }

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
        let mut fds = [0i32; 2];
        // SAFETY: pipe2 is POSIX.
        let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
        assert_eq!(ret, 0, "pipe2 failed");
        // SAFETY: fds are valid, we own them.
        unsafe {
            (
                std::fs::File::from_raw_fd(fds[0]),
                std::fs::File::from_raw_fd(fds[1]),
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
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        let path_clone = stderr_path.clone();
        let handle = std::thread::spawn(move || {
            run_stderr_drain(read_file, &path_clone, 1024 * 1024, true, shutdown_clone);
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
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        let max_bytes = 256u64;
        let path_clone = stderr_path.clone();
        let handle = std::thread::spawn(move || {
            run_stderr_drain(read_file, &path_clone, max_bytes, true, shutdown_clone);
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
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        let max_bytes = 128u64;
        let path_clone = stderr_path.clone();
        let handle = std::thread::spawn(move || {
            run_stderr_drain(read_file, &path_clone, max_bytes, false, shutdown_clone);
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
}
