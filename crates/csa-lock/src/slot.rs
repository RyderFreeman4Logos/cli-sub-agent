//! Global tool slot mechanism for system-wide concurrency control.
//!
//! Each tool has a configurable number of "slots" (default: 3).
//! A slot is a `flock(2)` advisory lock on a numbered file under
//! `~/.local/state/cli-sub-agent/slots/{tool}/slot-{NN}.lock`.
//!
//! Acquiring a slot means trying `flock(LOCK_EX | LOCK_NB)` on each file
//! in order until one succeeds. If all are occupied, the caller receives
//! a diagnostic snapshot to decide: wait, switch tools, or abort.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Diagnostic information written into each slot lock file.
#[derive(Debug, Serialize, Deserialize)]
struct SlotDiagnostic {
    pid: u32,
    tool_name: String,
    slot_index: u32,
    acquired_at: DateTime<Utc>,
    session_id: Option<String>,
}

/// Guard holding an acquired tool slot. Releases `flock` on drop.
pub struct ToolSlot {
    file: File,
    slot_path: PathBuf,
    tool_name: String,
    slot_index: u32,
    released: bool,
}

impl std::fmt::Debug for ToolSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolSlot")
            .field("tool_name", &self.tool_name)
            .field("slot_index", &self.slot_index)
            .field("slot_path", &self.slot_path)
            .finish()
    }
}

impl Drop for ToolSlot {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let fd = self.file.as_raw_fd();
        // SAFETY: `fd` is a valid file descriptor owned by `self.file`.
        // `LOCK_UN` releases the advisory lock. If this fails the lock
        // is released when the fd is closed moments later.
        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
        self.released = true;
    }
}

impl ToolSlot {
    /// The tool name for this slot.
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// The slot index (0-based).
    pub fn slot_index(&self) -> u32 {
        self.slot_index
    }

    /// Explicitly release this slot before drop.
    pub fn release_slot(&mut self) -> Result<()> {
        if self.released {
            return Ok(());
        }

        let fd = self.file.as_raw_fd();
        // SAFETY: `fd` is a valid file descriptor owned by `self.file`.
        let ret = unsafe { libc::flock(fd, libc::LOCK_UN) };
        if ret != 0 {
            anyhow::bail!("failed to release slot lock {}", self.slot_path.display());
        }
        self.released = true;
        Ok(())
    }
}

/// Diagnostic snapshot of slot usage for a single tool.
#[derive(Debug, Clone)]
pub struct SlotStatus {
    pub tool_name: String,
    pub max_slots: u32,
    pub occupied: u32,
}

impl SlotStatus {
    pub fn free(&self) -> u32 {
        self.max_slots.saturating_sub(self.occupied)
    }
}

/// Result of attempting to acquire a slot.
pub enum SlotAcquireResult {
    /// Successfully acquired a slot.
    Acquired(ToolSlot),
    /// All slots occupied; includes diagnostic info.
    Exhausted(SlotStatus),
}

/// Try to acquire a slot (non-blocking).
///
/// Iterates `slot-00` through `slot-{max-1}`, attempting
/// `flock(LOCK_EX | LOCK_NB)` on each. Returns the first available slot
/// or `Exhausted` with the number of occupied slots.
pub fn try_acquire_slot(
    slots_dir: &Path,
    tool_name: &str,
    max_concurrent: u32,
    session_id: Option<&str>,
) -> Result<SlotAcquireResult> {
    let tool_dir = slots_dir.join(tool_name);
    fs::create_dir_all(&tool_dir)
        .with_context(|| format!("Failed to create slot directory: {}", tool_dir.display()))?;

    for index in 0..max_concurrent {
        let slot_path = tool_dir.join(format!("slot-{:02}.lock", index));

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&slot_path)
            .with_context(|| format!("Failed to open slot file: {}", slot_path.display()))?;

        let fd = file.as_raw_fd();

        // SAFETY: `fd` is a valid file descriptor from the `File` we just opened.
        // `LOCK_EX | LOCK_NB` requests an exclusive non-blocking lock.
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

        if ret == 0 {
            // Slot acquired. Write diagnostic info.
            let mut slot = ToolSlot {
                file,
                slot_path,
                tool_name: tool_name.to_string(),
                slot_index: index,
                released: false,
            };

            let diagnostic = SlotDiagnostic {
                pid: std::process::id(),
                tool_name: tool_name.to_string(),
                slot_index: index,
                acquired_at: Utc::now(),
                session_id: session_id.map(|s| s.to_string()),
            };

            if let Ok(json) = serde_json::to_string(&diagnostic) {
                let _ = slot.file.set_len(0);
                let _ = slot.file.write_all(json.as_bytes());
                let _ = slot.file.flush();
            }

            return Ok(SlotAcquireResult::Acquired(slot));
        }
        // This slot is held; try the next one.
    }

    // All slots occupied.
    Ok(SlotAcquireResult::Exhausted(SlotStatus {
        tool_name: tool_name.to_string(),
        max_slots: max_concurrent,
        occupied: max_concurrent,
    }))
}

/// Block-wait for a slot with timeout.
///
/// If no slot is immediately available, blocks on `slot-00` with
/// `flock(LOCK_EX)` (blocking). Uses a poll loop with the given timeout.
pub fn acquire_slot_blocking(
    slots_dir: &Path,
    tool_name: &str,
    max_concurrent: u32,
    timeout: Duration,
    session_id: Option<&str>,
) -> Result<ToolSlot> {
    // First try non-blocking.
    match try_acquire_slot(slots_dir, tool_name, max_concurrent, session_id)? {
        SlotAcquireResult::Acquired(slot) => return Ok(slot),
        SlotAcquireResult::Exhausted(_) => {}
    }

    // Poll all slots in round-robin until one becomes free.
    let start = Instant::now();
    let mut sleep_ms = 100;

    loop {
        // Try every slot before sleeping.
        match try_acquire_slot(slots_dir, tool_name, max_concurrent, session_id)? {
            SlotAcquireResult::Acquired(slot) => return Ok(slot),
            SlotAcquireResult::Exhausted(_) => {}
        }

        if start.elapsed() >= timeout {
            anyhow::bail!(
                "Timed out waiting for slot '{}' after {:?}",
                tool_name,
                timeout
            );
        }

        std::thread::sleep(Duration::from_millis(sleep_ms));
        sleep_ms = (sleep_ms * 2).min(2000); // cap at 2s
    }
}

/// Get current slot usage for all tools (for diagnostics).
///
/// `tools` is a slice of `(tool_name, max_concurrent)` pairs.
pub fn slot_usage(slots_dir: &Path, tools: &[(&str, u32)]) -> Vec<SlotStatus> {
    tools
        .iter()
        .map(|(tool_name, max)| {
            let tool_dir = slots_dir.join(tool_name);
            let mut occupied = 0u32;

            for index in 0..*max {
                let slot_path = tool_dir.join(format!("slot-{:02}.lock", index));
                if let Ok(file) = OpenOptions::new().read(true).write(false).open(&slot_path) {
                    let fd = file.as_raw_fd();
                    // SAFETY: `fd` is valid. LOCK_EX | LOCK_NB to probe.
                    let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
                    if ret != 0 {
                        // Lock is held by another process.
                        occupied += 1;
                    } else {
                        // We acquired it; release immediately.
                        // SAFETY: fd is valid, LOCK_UN releases.
                        unsafe {
                            libc::flock(fd, libc::LOCK_UN);
                        }
                    }
                }
                // File doesn't exist → not occupied.
            }

            SlotStatus {
                tool_name: tool_name.to_string(),
                max_slots: *max,
                occupied,
            }
        })
        .collect()
}

/// Format a diagnostic message for slot exhaustion.
pub fn format_slot_diagnostic(
    tool_name: &str,
    status: &SlotStatus,
    all_usage: &[SlotStatus],
) -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "[csa:slot] {}: all {} slots occupied",
        tool_name, status.max_slots
    ));

    // Usage summary
    let usage_parts: Vec<String> = all_usage
        .iter()
        .map(|s| format!("{} {}/{}", s.tool_name, s.occupied, s.max_slots))
        .collect();
    lines.push(format!("[csa:slot] usage: {}", usage_parts.join(" | ")));

    // Alternatives with free slots
    let alternatives: Vec<String> = all_usage
        .iter()
        .filter(|s| s.tool_name != tool_name && s.free() > 0)
        .map(|s| format!("{} ({} free)", s.tool_name, s.free()))
        .collect();

    if !alternatives.is_empty() {
        lines.push(format!(
            "[csa:slot] alternatives: {}",
            alternatives.join(", ")
        ));
    }

    lines.push("[csa:slot] hint: --wait to block, or --tool <alt> to switch".to_string());

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_acquire_slot_succeeds() {
        let dir = tempdir().unwrap();
        let slots_dir = dir.path();

        let result = try_acquire_slot(slots_dir, "test-tool", 3, None).unwrap();
        assert!(matches!(result, SlotAcquireResult::Acquired(_)));

        if let SlotAcquireResult::Acquired(slot) = result {
            assert_eq!(slot.tool_name(), "test-tool");
            assert_eq!(slot.slot_index(), 0);
        }
    }

    #[test]
    fn test_acquire_multiple_slots() {
        let dir = tempdir().unwrap();
        let slots_dir = dir.path();

        let slot0 = try_acquire_slot(slots_dir, "test-tool", 3, None).unwrap();
        assert!(matches!(slot0, SlotAcquireResult::Acquired(_)));

        let slot1 = try_acquire_slot(slots_dir, "test-tool", 3, None).unwrap();
        assert!(matches!(slot1, SlotAcquireResult::Acquired(_)));

        let slot2 = try_acquire_slot(slots_dir, "test-tool", 3, None).unwrap();
        assert!(matches!(slot2, SlotAcquireResult::Acquired(_)));

        // Fourth should be exhausted
        let slot3 = try_acquire_slot(slots_dir, "test-tool", 3, None).unwrap();
        assert!(matches!(slot3, SlotAcquireResult::Exhausted(_)));

        if let SlotAcquireResult::Exhausted(status) = slot3 {
            assert_eq!(status.max_slots, 3);
            assert_eq!(status.occupied, 3);
            assert_eq!(status.free(), 0);
        }
    }

    #[test]
    fn test_different_tools_independent() {
        let dir = tempdir().unwrap();
        let slots_dir = dir.path();

        let _slot_a = try_acquire_slot(slots_dir, "tool-a", 1, None).unwrap();
        let slot_b = try_acquire_slot(slots_dir, "tool-b", 1, None).unwrap();

        // tool-a full, but tool-b should still work
        assert!(matches!(slot_b, SlotAcquireResult::Acquired(_)));
    }

    #[test]
    fn test_slot_diagnostic_written() {
        let dir = tempdir().unwrap();
        let slots_dir = dir.path();

        let result = try_acquire_slot(slots_dir, "test-tool", 3, Some("session-123")).unwrap();
        assert!(matches!(result, SlotAcquireResult::Acquired(_)));

        // Read the slot file
        let slot_path = slots_dir.join("test-tool/slot-00.lock");
        let content = fs::read_to_string(&slot_path).unwrap();
        let diag: SlotDiagnostic = serde_json::from_str(&content).unwrap();

        assert_eq!(diag.pid, std::process::id());
        assert_eq!(diag.tool_name, "test-tool");
        assert_eq!(diag.slot_index, 0);
        assert_eq!(diag.session_id.as_deref(), Some("session-123"));
    }

    #[test]
    fn test_slot_usage_empty() {
        let dir = tempdir().unwrap();
        let slots_dir = dir.path();

        let usage = slot_usage(slots_dir, &[("tool-a", 3), ("tool-b", 2)]);
        assert_eq!(usage.len(), 2);
        assert_eq!(usage[0].occupied, 0);
        assert_eq!(usage[1].occupied, 0);
    }

    #[test]
    fn test_format_slot_diagnostic() {
        let status = SlotStatus {
            tool_name: "codex".to_string(),
            max_slots: 3,
            occupied: 3,
        };
        let all_usage = vec![
            status.clone(),
            SlotStatus {
                tool_name: "opencode".to_string(),
                max_slots: 2,
                occupied: 1,
            },
            SlotStatus {
                tool_name: "claude-code".to_string(),
                max_slots: 1,
                occupied: 0,
            },
        ];

        let msg = format_slot_diagnostic("codex", &status, &all_usage);
        assert!(msg.contains("codex: all 3 slots occupied"));
        assert!(msg.contains("opencode (1 free)"));
        assert!(msg.contains("claude-code (1 free)"));
        assert!(msg.contains("--wait to block"));
    }

    #[test]
    fn test_slot_path_construction() {
        let dir = tempdir().unwrap();
        let slots_dir = dir.path();

        let result = try_acquire_slot(slots_dir, "my-tool", 3, None).unwrap();
        if let SlotAcquireResult::Acquired(slot) = result {
            let expected = slots_dir.join("my-tool").join("slot-00.lock");
            assert_eq!(slot.slot_path, expected);
        } else {
            panic!("expected Acquired");
        }
    }

    #[test]
    fn test_slot_path_index_padding() {
        let dir = tempdir().unwrap();
        let slots_dir = dir.path();

        // Acquire first slot so the second goes to index 1
        let _s0 = try_acquire_slot(slots_dir, "pad-tool", 10, None).unwrap();
        let result = try_acquire_slot(slots_dir, "pad-tool", 10, None).unwrap();
        if let SlotAcquireResult::Acquired(slot) = result {
            let expected = slots_dir.join("pad-tool").join("slot-01.lock");
            assert_eq!(slot.slot_path, expected);
        } else {
            panic!("expected Acquired at index 1");
        }
    }

    #[test]
    fn test_slot_usage_all_free_returns_zero() {
        let dir = tempdir().unwrap();
        let slots_dir = dir.path();

        // Create the tool directories but don't acquire any locks
        fs::create_dir_all(slots_dir.join("alpha")).unwrap();
        fs::create_dir_all(slots_dir.join("beta")).unwrap();

        let usage = slot_usage(slots_dir, &[("alpha", 5), ("beta", 2)]);
        assert_eq!(usage.len(), 2);
        for s in &usage {
            assert_eq!(s.occupied, 0, "{} should have 0 occupied", s.tool_name);
            assert_eq!(s.free(), s.max_slots);
        }
    }

    #[test]
    fn test_slot_status_free_saturating() {
        // Ensure `free()` never underflows even with bad data
        let status = SlotStatus {
            tool_name: "x".to_string(),
            max_slots: 0,
            occupied: 5,
        };
        assert_eq!(status.free(), 0);
    }

    #[test]
    fn test_acquire_slot_blocking_timeout() {
        let dir = tempdir().unwrap();
        let slots_dir = dir.path();

        // Exhaust the single available slot
        let _held = try_acquire_slot(slots_dir, "busy", 1, None).unwrap();

        let start = std::time::Instant::now();
        let result = acquire_slot_blocking(slots_dir, "busy", 1, Duration::from_millis(300), None);

        assert!(result.is_err(), "should timeout when all slots held");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Timed out"), "error: {err}");
        // Verify we actually waited (at least ~200ms given poll backoff)
        assert!(start.elapsed() >= Duration::from_millis(200));
    }

    #[test]
    fn test_acquire_slot_blocking_immediate_success() {
        let dir = tempdir().unwrap();
        let slots_dir = dir.path();

        // No slots held — should succeed without blocking
        let slot = acquire_slot_blocking(
            slots_dir,
            "fast-tool",
            2,
            Duration::from_secs(5),
            Some("sess-1"),
        )
        .unwrap();
        assert_eq!(slot.tool_name(), "fast-tool");
        assert_eq!(slot.slot_index(), 0);
    }

    #[test]
    fn test_format_slot_diagnostic_zero_slots() {
        let status = SlotStatus {
            tool_name: "empty".to_string(),
            max_slots: 0,
            occupied: 0,
        };
        let all_usage = vec![status.clone()];
        let msg = format_slot_diagnostic("empty", &status, &all_usage);
        assert!(msg.contains("empty: all 0 slots occupied"));
        // No alternatives line because no other tools
        assert!(!msg.contains("alternatives:"));
    }

    #[test]
    fn test_format_slot_diagnostic_all_tools_full() {
        let status_a = SlotStatus {
            tool_name: "a".to_string(),
            max_slots: 2,
            occupied: 2,
        };
        let status_b = SlotStatus {
            tool_name: "b".to_string(),
            max_slots: 1,
            occupied: 1,
        };
        let all_usage = vec![status_a.clone(), status_b];
        let msg = format_slot_diagnostic("a", &status_a, &all_usage);
        // No alternatives since b is also full
        assert!(!msg.contains("alternatives:"));
        assert!(msg.contains("--wait to block"));
    }

    #[test]
    fn test_try_acquire_slot_with_session_id_none() {
        let dir = tempdir().unwrap();
        let slots_dir = dir.path();

        let result = try_acquire_slot(slots_dir, "sid-tool", 1, None).unwrap();
        if let SlotAcquireResult::Acquired(_slot) = &result {
            let content = fs::read_to_string(slots_dir.join("sid-tool/slot-00.lock")).unwrap();
            let diag: SlotDiagnostic = serde_json::from_str(&content).unwrap();
            assert!(diag.session_id.is_none());
        } else {
            panic!("expected Acquired");
        }
    }

    #[test]
    fn test_exhausted_returns_correct_status() {
        let dir = tempdir().unwrap();
        let slots_dir = dir.path();

        let _s = try_acquire_slot(slots_dir, "one", 1, None).unwrap();
        let result = try_acquire_slot(slots_dir, "one", 1, None).unwrap();
        match result {
            SlotAcquireResult::Exhausted(st) => {
                assert_eq!(st.tool_name, "one");
                assert_eq!(st.max_slots, 1);
                assert_eq!(st.occupied, 1);
                assert_eq!(st.free(), 0);
            }
            _ => panic!("expected Exhausted"),
        }
    }

    #[test]
    fn test_release_slot_allows_reacquire_when_max_concurrent_is_one() {
        let dir = tempdir().unwrap();
        let slots_dir = dir.path();

        let mut first = match try_acquire_slot(slots_dir, "single", 1, None).unwrap() {
            SlotAcquireResult::Acquired(slot) => slot,
            SlotAcquireResult::Exhausted(_) => panic!("expected slot acquisition"),
        };

        first
            .release_slot()
            .expect("explicit release should succeed");

        let second = try_acquire_slot(slots_dir, "single", 1, None).unwrap();
        assert!(
            matches!(second, SlotAcquireResult::Acquired(_)),
            "slot should be reacquired after explicit release"
        );
    }
}
