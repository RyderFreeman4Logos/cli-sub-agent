//! POSIX `setrlimit` enforcement and RSS monitoring fallback.
//!
//! Provides two complementary mechanisms:
//! 1. [`apply_rlimits`] — sets `RLIMIT_AS` (address space) and optionally
//!    `RLIMIT_NPROC` on the **current process**.  Intended to be called in a
//!    child process after fork (e.g. via `Command::pre_exec`).
//! 2. [`RssWatcher`] — lightweight background thread that periodically reads
//!    `/proc/{pid}/statm` and emits warnings or sends `SIGTERM` when RSS
//!    exceeds configured thresholds.  This is a secondary safety net; the
//!    primary enforcement is `RLIMIT_AS`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::warn;

/// Apply `RLIMIT_AS` and optionally `RLIMIT_NPROC` to the current process.
///
/// # Arguments
/// * `memory_max_mb` — maximum virtual address-space size in megabytes.
/// * `pids_max` — optional maximum number of processes (NPROC limit).
///
/// # Safety note
/// This function uses `libc::setrlimit` which is async-signal-safe on Linux,
/// making it suitable for use inside a `pre_exec` closure.
pub fn apply_rlimits(memory_max_mb: u64, pids_max: Option<u64>) -> Result<()> {
    let as_bytes = memory_max_mb
        .checked_mul(1024 * 1024)
        .context("memory_max_mb overflow when converting to bytes")?;

    let rlim_as = libc::rlimit {
        rlim_cur: as_bytes,
        rlim_max: as_bytes,
    };

    // SAFETY: setrlimit is a well-defined POSIX syscall; we pass a valid
    // rlimit struct for RLIMIT_AS.
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_AS, &rlim_as) };
    if ret != 0 {
        return Err(std::io::Error::last_os_error())
            .context(format!("setrlimit(RLIMIT_AS, {} MB) failed", memory_max_mb));
    }

    if let Some(nproc) = pids_max {
        let rlim_nproc = libc::rlimit {
            rlim_cur: nproc,
            rlim_max: nproc,
        };

        // SAFETY: same rationale as above for RLIMIT_NPROC.
        let ret = unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &rlim_nproc) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error())
                .context(format!("setrlimit(RLIMIT_NPROC, {}) failed", nproc));
        }
    }

    Ok(())
}

/// Background thread that monitors a child process's RSS via `/proc/{pid}/statm`.
///
/// Emits a `tracing::warn` at 90% of the memory threshold and sends `SIGTERM`
/// to the process group if RSS reaches 100%.  Implements [`Drop`] to
/// automatically stop the watcher thread.
pub struct RssWatcher {
    stop_flag: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl RssWatcher {
    /// Start watching the given PID.
    ///
    /// * `pid` — OS process ID to monitor.
    /// * `memory_max_mb` — threshold in megabytes.
    /// * `poll_interval` — how often to sample (recommended: 5 s).
    pub fn start(pid: u32, memory_max_mb: u64, poll_interval: Duration) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);

        let handle = thread::Builder::new()
            .name(format!("rss-watcher-{pid}"))
            .spawn(move || Self::watch_loop(pid, memory_max_mb, poll_interval, stop))
            .context("failed to spawn rss-watcher thread")?;

        Ok(Self {
            stop_flag,
            handle: Some(handle),
        })
    }

    /// Core polling loop executed on the background thread.
    fn watch_loop(pid: u32, memory_max_mb: u64, interval: Duration, stop: Arc<AtomicBool>) {
        let warn_threshold_pages = mb_to_pages(memory_max_mb * 90 / 100);
        let kill_threshold_pages = mb_to_pages(memory_max_mb);
        let mut warned = false;

        while !stop.load(Ordering::Relaxed) {
            thread::sleep(interval);
            if stop.load(Ordering::Relaxed) {
                break;
            }

            let rss_pages = match read_rss_pages(pid) {
                Some(p) => p,
                // Process exited or /proc entry gone — stop watching.
                None => break,
            };

            if rss_pages >= kill_threshold_pages {
                warn!(
                    pid,
                    rss_mb = pages_to_mb(rss_pages),
                    limit_mb = memory_max_mb,
                    "RSS exceeds limit — sending SIGTERM to process group"
                );
                send_sigterm_to_group(pid);
                break;
            }

            if rss_pages >= warn_threshold_pages && !warned {
                warn!(
                    pid,
                    rss_mb = pages_to_mb(rss_pages),
                    limit_mb = memory_max_mb,
                    "RSS approaching limit (>90%)"
                );
                warned = true;
            }
        }
    }

    /// Gracefully stop the watcher without sending any signal.
    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for RssWatcher {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        // Don't join in Drop — the thread will notice the flag on next poll.
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read RSS (resident set size) in pages from `/proc/{pid}/statm`.
///
/// Field layout: `size resident shared text lib data dt`
/// We want field index 1 (resident).
fn read_rss_pages(pid: u32) -> Option<u64> {
    let path = format!("/proc/{pid}/statm");
    let content = std::fs::read_to_string(path).ok()?;
    let rss_str = content.split_whitespace().nth(1)?;
    rss_str.parse::<u64>().ok()
}

/// Convert megabytes to pages (assuming 4 KiB page size).
fn mb_to_pages(mb: u64) -> u64 {
    let page_size = page_size_bytes();
    mb.saturating_mul(1024 * 1024) / page_size
}

/// Convert pages to megabytes (assuming 4 KiB page size).
fn pages_to_mb(pages: u64) -> u64 {
    let page_size = page_size_bytes();
    pages.saturating_mul(page_size) / 1024 / 1024
}

/// Query the system page size (typically 4096).
fn page_size_bytes() -> u64 {
    // SAFETY: sysconf is a standard POSIX function with no side effects.
    let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if ps <= 0 { 4096 } else { ps as u64 }
}

/// Send `SIGTERM` to the process **group** (negative PID).
fn send_sigterm_to_group(pid: u32) {
    // SAFETY: kill(-pid, SIGTERM) targets the process group.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGTERM);
    }
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
    fn test_page_size_positive() {
        assert!(page_size_bytes() > 0);
    }

    #[test]
    fn test_mb_to_pages_roundtrip() {
        let mb = 256;
        let pages = mb_to_pages(mb);
        let back = pages_to_mb(pages);
        assert_eq!(back, mb);
    }

    #[test]
    fn test_read_rss_pages_self() {
        // Reading our own process should succeed on Linux.
        if cfg!(target_os = "linux") {
            let pid = std::process::id();
            let rss = read_rss_pages(pid);
            assert!(rss.is_some(), "should be able to read own RSS");
            assert!(rss.unwrap() > 0, "RSS should be positive");
        }
    }

    #[test]
    fn test_read_rss_pages_nonexistent() {
        // PID that almost certainly doesn't exist.
        assert!(read_rss_pages(u32::MAX - 1).is_none());
    }

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
    fn test_apply_rlimits_overflow() {
        let result = apply_rlimits(u64::MAX, None);
        assert!(result.is_err(), "should detect overflow");
    }

    #[test]
    fn test_rss_watcher_stop() {
        // Start watcher for a nonexistent PID — it should exit quickly.
        let watcher = RssWatcher::start(u32::MAX - 1, 1024, Duration::from_millis(50))
            .expect("thread spawn should succeed in test");
        // Give the thread time to detect missing PID.
        thread::sleep(Duration::from_millis(200));
        watcher.stop();
    }

    #[test]
    fn test_rss_watcher_drop_does_not_panic() {
        let _watcher = RssWatcher::start(u32::MAX - 1, 1024, Duration::from_millis(50))
            .expect("thread spawn should succeed in test");
        // Dropping without explicit stop should not panic.
    }
}
