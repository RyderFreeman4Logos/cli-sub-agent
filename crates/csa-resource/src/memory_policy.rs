//! Shared memory-limit policy used before spawn and during runtime monitoring.

/// Default soft memory limit as a percentage of `MemoryMax`.
///
/// Lowered from 80% to 70% in #568 to provide headroom before SIGTERM on
/// memory-constrained hosts. Raised from 65% to 70% after review found that
/// 65% of Codex's 12288MB limit (7987MB) was below the old 8192MB hard cap,
/// effectively negating the #555 increase. (Codex's default was later raised
/// to 16384MB in #2650.)
pub const DEFAULT_SOFT_LIMIT_PERCENT: u8 = 70;

/// Return the soft-limit threshold in MB for a configured memory cap.
pub fn soft_limit_threshold_mb(memory_max_mb: u64, soft_limit_percent: u8) -> Option<u64> {
    soft_limit_threshold(memory_max_mb, soft_limit_percent)
}

/// Return the soft-limit threshold in bytes for a configured memory cap.
pub fn soft_limit_threshold_bytes(memory_max_bytes: u64, soft_limit_percent: u8) -> Option<u64> {
    soft_limit_threshold(memory_max_bytes, soft_limit_percent)
}

/// Return the minimum memory cap in MB required to satisfy a soft-limit floor.
pub fn required_memory_max_for_soft_limit_mb(
    required_threshold_mb: u64,
    soft_limit_percent: u8,
) -> Option<u64> {
    if !is_valid_soft_limit_percent(soft_limit_percent) {
        return None;
    }

    let percent = u64::from(soft_limit_percent);
    Some(
        required_threshold_mb
            .saturating_mul(100)
            .saturating_add(percent.saturating_sub(1))
            / percent,
    )
}

fn soft_limit_threshold(memory_max: u64, soft_limit_percent: u8) -> Option<u64> {
    if memory_max == 0 || !is_valid_soft_limit_percent(soft_limit_percent) {
        return None;
    }

    Some(memory_max.saturating_mul(u64::from(soft_limit_percent)) / 100)
}

fn is_valid_soft_limit_percent(soft_limit_percent: u8) -> bool {
    (1..=100).contains(&soft_limit_percent)
}
