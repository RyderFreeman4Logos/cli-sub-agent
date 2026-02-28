//! Memory balloon for swap-pressure testing via anonymous mmap.
//!
//! [`MemoryBalloon`] allocates a contiguous block of anonymous memory using
//! `mmap(MAP_ANONYMOUS | MAP_PRIVATE | MAP_POPULATE)`.  The `MAP_POPULATE`
//! flag forces the kernel to fault in all pages immediately, ensuring the
//! allocation actually consumes physical memory (or swap).  On [`Drop`],
//! the mapping is released via `munmap`.
//!
//! Use [`should_enable_balloon`] to check whether the system has sufficient
//! swap headroom before inflating.

use anyhow::{Context, Result, bail};

/// Hard upper limit: 16 GiB.  Prevents accidental OOM from absurd values.
const MAX_BALLOON_SIZE: u64 = 16 * 1024 * 1024 * 1024;

/// Anonymous mmap-backed memory balloon.
///
/// Inflates on construction, deflates on drop.  The mapping is `PROT_READ |
/// PROT_WRITE` so the kernel commits physical pages (via `MAP_POPULATE`).
pub struct MemoryBalloon {
    ptr: *mut libc::c_void,
    size: usize,
}

impl std::fmt::Debug for MemoryBalloon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryBalloon")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

// SAFETY: The mmap region is private (MAP_PRIVATE) and owned exclusively by
// this struct.  No aliasing or shared mutation is possible.
unsafe impl Send for MemoryBalloon {}
unsafe impl Sync for MemoryBalloon {}

impl MemoryBalloon {
    /// Inflate a balloon of exactly `size_bytes` bytes.
    ///
    /// Returns an error if `size_bytes` exceeds [`MAX_BALLOON_SIZE`] (16 GiB),
    /// is zero, or if the `mmap` syscall fails.
    pub fn inflate(size_bytes: usize) -> Result<Self> {
        if size_bytes == 0 {
            bail!("balloon size must be > 0");
        }

        if size_bytes as u64 > MAX_BALLOON_SIZE {
            bail!(
                "balloon size {} bytes exceeds hard limit of {} bytes (16 GiB)",
                size_bytes,
                MAX_BALLOON_SIZE
            );
        }

        // SAFETY: mmap with MAP_ANONYMOUS does not require a file descriptor.
        // We pass -1 for fd and 0 for offset.  The returned pointer is either
        // MAP_FAILED (checked below) or a valid mapping of `size_bytes` length.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size_bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANONYMOUS | libc::MAP_PRIVATE | libc::MAP_POPULATE,
                -1,
                0,
            )
        };

        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error())
                .context(format!("mmap({} bytes) failed", size_bytes));
        }

        Ok(Self {
            ptr,
            size: size_bytes,
        })
    }

    /// Size of the inflated balloon in bytes.
    pub fn size(&self) -> usize {
        self.size
    }
}

impl Drop for MemoryBalloon {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` was returned by a successful mmap call and
        // `self.size` matches the original mapping length.  After munmap
        // the pointer is invalid — but we never use it again.
        let ret = unsafe { libc::munmap(self.ptr, self.size) };
        if ret != 0 {
            // Best-effort warning; cannot propagate errors from Drop.
            tracing::warn!(
                size = self.size,
                err = %std::io::Error::last_os_error(),
                "munmap failed during balloon deflation"
            );
        }
    }
}

/// Check whether a memory balloon should be enabled given current swap
/// availability.
///
/// Returns `true` only when:
/// 1. `balloon_size` does not exceed [`MAX_BALLOON_SIZE`] (16 GiB), AND
/// 2. `available_swap_bytes` is at least as large as `balloon_size`.
pub fn should_enable_balloon(available_swap_bytes: u64, balloon_size: u64) -> bool {
    balloon_size <= MAX_BALLOON_SIZE && available_swap_bytes >= balloon_size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_balloon_size_clamp() {
        // Exactly at limit should succeed.
        let at_limit = MAX_BALLOON_SIZE as usize;
        // We don't actually allocate 16 GiB in tests; just verify the
        // validation logic by testing one byte over.
        let over = at_limit.saturating_add(1);
        let result = MemoryBalloon::inflate(over);
        assert!(result.is_err(), "should reject size > 16 GiB");
        let err_msg = format!("{:#}", result.unwrap_err());
        assert!(
            err_msg.contains("hard limit"),
            "error should mention hard limit, got: {err_msg}"
        );
    }

    #[test]
    fn test_balloon_enable_conditions() {
        // Enough swap.
        assert!(should_enable_balloon(
            2 * 1024 * 1024 * 1024,
            1024 * 1024 * 1024
        ));

        // Exactly equal.
        assert!(should_enable_balloon(1024, 1024));

        // Not enough swap.
        assert!(!should_enable_balloon(512, 1024));

        // Balloon exceeds hard limit.
        assert!(!should_enable_balloon(u64::MAX, MAX_BALLOON_SIZE + 1));

        // Zero balloon is fine for the condition check (separate from inflate).
        assert!(should_enable_balloon(0, 0));
    }

    #[test]
    fn test_balloon_mmap_failure_graceful() {
        // Size 0 is rejected before reaching mmap.
        let result = MemoryBalloon::inflate(0);
        assert!(result.is_err(), "size 0 should be rejected");
        let err_msg = format!("{:#}", result.unwrap_err());
        assert!(
            err_msg.contains("must be > 0"),
            "error should mention zero size, got: {err_msg}"
        );
    }

    #[test]
    fn test_balloon_inflate_deflate() {
        let one_mb = 1024 * 1024;
        let balloon = MemoryBalloon::inflate(one_mb).expect("1 MiB balloon should succeed");
        assert_eq!(balloon.size(), one_mb);

        // Write a byte to verify the mapping is usable.
        // SAFETY: the mapping is PROT_READ | PROT_WRITE and at least 1 byte.
        unsafe {
            std::ptr::write_volatile(balloon.ptr as *mut u8, 0xAB);
            let val = std::ptr::read_volatile(balloon.ptr as *const u8);
            assert_eq!(val, 0xAB);
        }

        // Explicit drop to verify no panic / no leak.
        drop(balloon);
    }

    #[test]
    fn test_balloon_drop_cleanup() {
        // Allocate and immediately drop — munmap should succeed silently.
        let balloon = MemoryBalloon::inflate(4096).expect("page-sized balloon should succeed");
        let ptr = balloon.ptr;
        let size = balloon.size;
        drop(balloon);

        // After drop, accessing the mapping would be UB, so we only verify
        // the drop didn't panic.  As a secondary check, attempt mmap at the
        // same address (hint) — if munmap worked, the kernel may reuse it.
        // SAFETY: mmap with a hint address is always safe; kernel may ignore it.
        let new_ptr = unsafe {
            libc::mmap(
                ptr,
                size,
                libc::PROT_READ,
                libc::MAP_ANONYMOUS | libc::MAP_PRIVATE,
                -1,
                0,
            )
        };
        if new_ptr != libc::MAP_FAILED {
            // SAFETY: valid mapping from successful mmap.
            unsafe {
                libc::munmap(new_ptr, size);
            }
        }
        // If we got here without panic or signal, cleanup succeeded.
    }
}
