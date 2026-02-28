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

/// Read current RLIMIT_AS soft limit (useful for `csa doctor` output).
pub fn current_rlimit_as() -> Option<u64> {
    let mut rlim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit is a well-defined POSIX syscall.
    let ret = unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut rlim) };
    if ret != 0 {
        return None;
    }
    if rlim.rlim_cur == libc::RLIM_INFINITY {
        None
    } else {
        Some(rlim.rlim_cur / 1024 / 1024)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_rlimit_as_runs() {
        // Just ensure it doesn't panic.
        let _ = current_rlimit_as();
    }

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
}
