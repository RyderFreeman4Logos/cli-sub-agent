//! Audit event persistence for merge guard and other enforcement gates.
//!
//! Events are appended to JSONL (one JSON object per line) files in
//! `~/.local/state/cli-sub-agent/events/`. This provides a durable,
//! machine-readable audit trail without requiring a database.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use tracing::debug;

/// Directory name under CSA's state dir for audit event logs.
const EVENTS_DIR_NAME: &str = "events";

/// JSONL file name for merge guard audit events.
const MERGE_GUARD_LOG: &str = "merge-guard.jsonl";

/// Audit event emitted when merge_guard allows a merge to proceed.
#[derive(Debug, Clone, Serialize)]
pub struct MergeAuditEvent {
    pub event: &'static str,
    pub pr_number: u64,
    pub head_sha: String,
    pub marker_path: PathBuf,
    pub timestamp: String,
}

/// Emit a `MergeCompleted` audit event to the JSONL log.
///
/// Appends one JSON line per successful merge verification. The log file
/// is at `~/.local/state/cli-sub-agent/events/merge-guard.jsonl`.
///
/// Best-effort: errors are logged but never propagated.
pub fn emit_merge_completed_event(pr_number: u64, head_sha: &str, marker_path: &Path) {
    if let Err(err) = emit_merge_completed_event_inner(pr_number, head_sha, marker_path) {
        tracing::warn!(
            pr_number,
            error = %err,
            "failed to emit MergeCompleted audit event (non-fatal)"
        );
    }
}

fn emit_merge_completed_event_inner(
    pr_number: u64,
    head_sha: &str,
    marker_path: &Path,
) -> Result<()> {
    let state_dir =
        csa_config::paths::state_dir().context("cannot determine CSA state directory")?;
    let events_dir = state_dir.join(EVENTS_DIR_NAME);
    fs::create_dir_all(&events_dir)
        .with_context(|| format!("failed to create events dir: {}", events_dir.display()))?;

    let event = MergeAuditEvent {
        event: "MergeCompleted",
        pr_number,
        head_sha: head_sha.to_string(),
        marker_path: marker_path.to_path_buf(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    let log_path = events_dir.join(MERGE_GUARD_LOG);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open audit log: {}", log_path.display()))?;

    let mut line = serde_json::to_string(&event).context("failed to serialize audit event")?;
    line.push('\n');
    file.write_all(line.as_bytes())
        .with_context(|| format!("failed to write audit event to {}", log_path.display()))?;

    debug!(
        pr_number,
        head_sha,
        log = %log_path.display(),
        "MergeCompleted audit event emitted"
    );
    Ok(())
}

/// Return the JSONL audit log path for merge guard events.
pub fn audit_log_path() -> Option<PathBuf> {
    Some(
        csa_config::paths::state_dir()?
            .join(EVENTS_DIR_NAME)
            .join(MERGE_GUARD_LOG),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;

    /// RAII guard that sets `XDG_STATE_HOME` to a temp path and restores
    /// the original value on drop.
    struct ScopedXdgOverride {
        orig: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl ScopedXdgOverride {
        fn new(tmp: &tempfile::TempDir) -> Self {
            let lock = ENV_LOCK.lock().expect("env lock poisoned");
            let orig = std::env::var("XDG_STATE_HOME").ok();
            // SAFETY: test-scoped env mutation protected by AUDIT_ENV_LOCK.
            unsafe {
                std::env::set_var("XDG_STATE_HOME", tmp.path().join("state").to_str().unwrap());
            }
            Self { orig, _lock: lock }
        }
    }

    impl Drop for ScopedXdgOverride {
        fn drop(&mut self) {
            // SAFETY: restoration of test-scoped env mutation (lock still held).
            unsafe {
                match &self.orig {
                    Some(v) => std::env::set_var("XDG_STATE_HOME", v),
                    None => std::env::remove_var("XDG_STATE_HOME"),
                }
            }
        }
    }

    #[test]
    fn test_emit_merge_completed_event_writes_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let _xdg = ScopedXdgOverride::new(&tmp);

        emit_merge_completed_event(
            42,
            "abc123def",
            Path::new("/tmp/markers/owner_repo/42-abc123def.done"),
        );

        let log_path = csa_config::paths::state_dir()
            .expect("state dir")
            .join("events/merge-guard.jsonl");
        assert!(log_path.exists(), "audit log should be created");

        let content = fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1, "should have exactly one event line");

        let event: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(event["event"], "MergeCompleted");
        assert_eq!(event["pr_number"], 42);
        assert_eq!(event["head_sha"], "abc123def");
        assert!(event["timestamp"].as_str().unwrap().contains('T'));
    }

    #[test]
    fn test_emit_merge_completed_event_appends() {
        let tmp = tempfile::tempdir().unwrap();
        let _xdg = ScopedXdgOverride::new(&tmp);

        emit_merge_completed_event(1, "sha1", Path::new("/tmp/m1.done"));
        emit_merge_completed_event(2, "sha2", Path::new("/tmp/m2.done"));

        let log_path = csa_config::paths::state_dir()
            .expect("state dir")
            .join("events/merge-guard.jsonl");
        let content = fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2, "should have two event lines");

        let e1: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let e2: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(e1["pr_number"], 1);
        assert_eq!(e2["pr_number"], 2);
    }

    #[test]
    fn test_audit_log_path_returns_expected_location() {
        let path = audit_log_path();
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.ends_with("events/merge-guard.jsonl"));
    }

    #[test]
    fn test_merge_audit_event_serialization() {
        let event = MergeAuditEvent {
            event: "MergeCompleted",
            pr_number: 99,
            head_sha: "deadbeef".to_string(),
            marker_path: PathBuf::from("/tmp/markers/99-deadbeef.done"),
            timestamp: "2026-04-04T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"MergeCompleted\""));
        assert!(json.contains("\"pr_number\":99"));
        assert!(json.contains("\"head_sha\":\"deadbeef\""));
        assert!(json.contains("\"timestamp\":\"2026-04-04T00:00:00Z\""));
    }
}
