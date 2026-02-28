//! POSIX `setrlimit` enforcement for PID limits.
//!
//! Provides [`apply_rlimits`] which optionally sets `RLIMIT_NPROC` on the
//! **current process**.  Intended to be called in a child process after fork
//! (e.g. via `Command::pre_exec`).
//!
//! Memory enforcement is handled by cgroup scopes (preferred) or
//! [`MemoryBalloon`](super::memory_balloon) (fallback); RLIMIT_AS is no
//! longer used because it conflicts with allocator overcommit and causes
//! spurious ENOMEM in well-behaved processes.

use anyhow::{Context, Result};

/// Apply optional `RLIMIT_NPROC` to the current process.
///
/// # Arguments
/// * `_memory_max_mb` — Ignored.  Retained for API compatibility with callers
///   that still pass memory limits (e.g. `csa-mcp-hub`).  Memory enforcement
///   is now handled outside `setrlimit`.
/// * `pids_max` — optional maximum number of processes (NPROC limit).
///
/// # Safety note
/// This function uses `libc::setrlimit` which is async-signal-safe on Linux,
/// making it suitable for use inside a `pre_exec` closure.
pub fn apply_rlimits(_memory_max_mb: u64, pids_max: Option<u64>) -> Result<()> {
    if let Some(nproc) = pids_max {
        let rlim_nproc = libc::rlimit {
            rlim_cur: nproc,
            rlim_max: nproc,
        };

        // SAFETY: setrlimit is a well-defined POSIX syscall; we pass a valid
        // rlimit struct for RLIMIT_NPROC.
        let ret = unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &rlim_nproc) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error())
                .context(format!("setrlimit(RLIMIT_NPROC, {}) failed", nproc));
        }
    }

    Ok(())
}

/// Read current RLIMIT_NPROC soft limit.
pub fn current_rlimit_nproc() -> Option<u64> {
    let mut rlim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit is a well-defined POSIX syscall.
    let ret = unsafe { libc::getrlimit(libc::RLIMIT_NPROC, &mut rlim) };
    if ret != 0 {
        return None;
    }
    if rlim.rlim_cur == libc::RLIM_INFINITY {
        None
    } else {
        Some(rlim.rlim_cur)
    }
}

/// Raise the OOM score adjustment for the current process.
///
/// Writes `+500` to `/proc/self/oom_score_adj` so that the OOM killer
/// prefers this process over system-critical services when memory is
/// exhausted.  This is a best-effort fallback used when neither cgroup
/// scopes nor `setrlimit` are available.
///
/// Intended to be called in a child process after fork (e.g. via
/// `Command::pre_exec`).  Writing to `/proc/self/` at that point
/// affects only the child.
///
/// Returns `Ok(())` on success or if `/proc/self/oom_score_adj` does
/// not exist (non-Linux).  Returns `Err` only on unexpected I/O errors.
pub fn apply_oom_score_adj() -> Result<()> {
    // Use raw libc syscalls instead of std::fs to be async-signal-safe.
    // This function runs inside a pre_exec closure (after fork, before exec)
    // where only async-signal-safe functions may be called.
    const PATH: &[u8] = b"/proc/self/oom_score_adj\0";
    const VALUE: &[u8] = b"500";

    // SAFETY: open() is async-signal-safe per POSIX.  We pass a valid
    // NUL-terminated path and O_WRONLY flag.
    let fd = unsafe { libc::open(PATH.as_ptr().cast::<libc::c_char>(), libc::O_WRONLY) };
    if fd < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ENOENT) || err.raw_os_error() == Some(libc::EACCES) {
            // Non-Linux, procfs not mounted, or container restriction — skip.
            return Ok(());
        }
        return Err(err).context("failed to open /proc/self/oom_score_adj");
    }

    // SAFETY: write() is async-signal-safe.  fd is a valid open descriptor.
    let written = unsafe { libc::write(fd, VALUE.as_ptr().cast::<libc::c_void>(), VALUE.len()) };

    // SAFETY: close() is async-signal-safe.
    unsafe { libc::close(fd) };

    if written < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EACCES) {
            // Inside some containers the file is read-only.
            return Ok(());
        }
        return Err(err).context("failed to write /proc/self/oom_score_adj");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_rlimit_nproc_runs() {
        let _ = current_rlimit_nproc();
    }

    #[test]
    fn test_apply_rlimits_nproc_only() {
        // With memory_max_mb ignored and no pids_max, should be a no-op.
        let result = apply_rlimits(1024, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_apply_oom_score_adj_succeeds() {
        // On Linux with /proc mounted, this should succeed (or silently
        // skip if permission is denied inside a container).
        let result = apply_oom_score_adj();
        assert!(result.is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_oom_score_adj_set_when_no_cgroup() {
        // Verify that apply_oom_score_adj actually writes to the procfs file.
        // Read the value before and after to confirm the write took effect.
        // Note: this modifies the current process's OOM score, which is
        // acceptable in test since the test runner's score is non-critical.
        use std::fs;

        let path = "/proc/self/oom_score_adj";
        let before = fs::read_to_string(path)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok());

        let result = apply_oom_score_adj();
        assert!(result.is_ok(), "apply_oom_score_adj should succeed");

        let after = fs::read_to_string(path)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok());

        // If we could read the file, verify the value was set.
        if let (Some(_), Some(after_val)) = (before, after) {
            assert_eq!(after_val, 500, "OOM score adj should be set to 500");
        }
    }
}
