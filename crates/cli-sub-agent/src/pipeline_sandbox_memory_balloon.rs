use std::path::Path;

use csa_resource::memory_balloon::{MemoryBalloon, should_enable_balloon};
use tracing::{info, warn};

/// Conditionally inflate and immediately deflate a memory balloon for claude-code.
///
/// Pre-warms RAM by `mmap`-ing a large anonymous mapping with `MAP_POPULATE`, forcing
/// the kernel to swap out other processes. The balloon is dropped right away so the
/// freed physical pages are available for the tool process about to launch.
pub(crate) fn maybe_inflate_balloon(tool_name: &str, project_root: &Path, session_id: &str) {
    if tool_name != "claude-code" {
        return;
    }

    const BALLOON_SIZE: usize = 1024 * 1024 * 1024; // 1 GiB
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let available_memory = sys.available_memory();
    let available_swap = sys.free_swap();
    let available_memory_mb = available_memory / 1024 / 1024;
    let active_session_count =
        crate::resource_admission::active_session_count_for_balloon(project_root, session_id);

    if should_skip_balloon_prewarm(available_memory_mb, active_session_count) {
        warn!(
            available_memory_mb,
            active_session_count,
            "Skipping MemoryBalloon prewarm under host memory or concurrent-session pressure"
        );
        return;
    }

    if !should_enable_balloon(available_memory, available_swap, BALLOON_SIZE as u64) {
        return;
    }

    match MemoryBalloon::inflate(BALLOON_SIZE) {
        Ok(balloon) => {
            info!(
                size_mb = BALLOON_SIZE / 1024 / 1024,
                "Memory balloon inflated — deflating immediately"
            );
            drop(balloon);
        }
        Err(e) => {
            // Balloon is an optimisation; failure is non-fatal.
            warn!(error = %e, "Memory balloon inflation failed; continuing without pre-warming");
        }
    }
}

const BALLOON_MIN_AVAILABLE_MEMORY_MB: u64 = 8192;

pub(crate) fn should_skip_balloon_prewarm(
    available_memory_mb: u64,
    active_session_count: u64,
) -> bool {
    available_memory_mb < BALLOON_MIN_AVAILABLE_MEMORY_MB || active_session_count > 0
}
