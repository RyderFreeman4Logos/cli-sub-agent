//! Session cooldown enforcement.
//!
//! Prevents rapid successive session launches by enforcing a configurable
//! cooldown period between completions. The marker file is written on session
//! completion and checked on session creation.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Policy & action enums
// ---------------------------------------------------------------------------

/// Result of evaluating whether cooldown is needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CooldownAction {
    /// No cooldown needed, proceed immediately.
    Proceed,
    /// Wait for the specified duration before proceeding.
    Wait(Duration),
}

// ---------------------------------------------------------------------------
// Pure evaluation
// ---------------------------------------------------------------------------

/// Determine whether the caller must wait before launching a new session.
///
/// This is a pure function — all IO (reading the marker, obtaining `now`) is
/// the caller's responsibility.
pub fn evaluate_cooldown(
    last_completed: Option<DateTime<Utc>>,
    cooldown_seconds: u64,
    now: DateTime<Utc>,
) -> CooldownAction {
    if cooldown_seconds == 0 {
        return CooldownAction::Proceed;
    }

    let completed = match last_completed {
        Some(ts) => ts,
        None => return CooldownAction::Proceed,
    };

    let elapsed = now.signed_duration_since(completed);
    let cooldown = chrono::TimeDelta::seconds(cooldown_seconds as i64);

    if elapsed >= cooldown {
        CooldownAction::Proceed
    } else {
        let remaining = cooldown - elapsed;
        // remaining is guaranteed positive here; clamp to zero just in case.
        let millis = remaining.num_milliseconds().max(0) as u64;
        CooldownAction::Wait(Duration::from_millis(millis))
    }
}

// ---------------------------------------------------------------------------
// Marker file I/O
// ---------------------------------------------------------------------------

const MARKER_FILENAME: &str = "cooldown-marker.toml";

/// Persistent marker recording when the last session completed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CooldownMarker {
    pub session_id: String,
    pub completed_at: DateTime<Utc>,
}

/// Read the cooldown marker for a project's session directory.
///
/// Returns `None` when the file is missing or unparseable (with a warning).
pub fn read_cooldown_marker(project_sessions_dir: &Path) -> Option<CooldownMarker> {
    let path = project_sessions_dir.join(MARKER_FILENAME);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("failed to read cooldown marker at {}: {e}", path.display());
            return None;
        }
    };

    match toml::from_str::<CooldownMarker>(&content) {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::warn!("failed to parse cooldown marker at {}: {e}", path.display());
            None
        }
    }
}

/// Write (or overwrite) the cooldown marker atomically.
///
/// Uses [`tempfile::NamedTempFile`] + [`persist`](tempfile::NamedTempFile::persist)
/// for atomic writes. The temp file is automatically cleaned up on `Drop` if
/// `persist` is never reached (e.g. write failure), preventing orphaned `.tmp`
/// files on error paths.
///
/// # Errors
///
/// Returns an error if:
/// - The parent directory cannot be created.
/// - The marker fails to serialize to TOML.
/// - The temporary file cannot be created or written.
/// - The atomic persist (rename) to the final path fails.
pub fn write_cooldown_marker(
    project_sessions_dir: &Path,
    session_id: &str,
    completed_at: DateTime<Utc>,
) -> Result<()> {
    std::fs::create_dir_all(project_sessions_dir)
        .with_context(|| format!("creating dir {}", project_sessions_dir.display()))?;

    let marker = CooldownMarker {
        session_id: session_id.to_owned(),
        completed_at,
    };

    let content = toml::to_string_pretty(&marker).context("serializing cooldown marker to TOML")?;

    let mut tmp = tempfile::NamedTempFile::new_in(project_sessions_dir)
        .with_context(|| format!("creating temp file in {}", project_sessions_dir.display()))?;
    tmp.write_all(content.as_bytes())
        .with_context(|| "writing cooldown marker to temp file")?;

    let final_path = project_sessions_dir.join(MARKER_FILENAME);
    tmp.persist(&final_path)
        .with_context(|| format!("persisting cooldown marker to {}", final_path.display()))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// High-level enforcement helper
// ---------------------------------------------------------------------------

/// Compute cooldown wait duration, accounting for skip conditions.
///
/// Returns `None` (proceed) when: `cooldown_seconds == 0`, resume
/// (`session_arg` set), fork (`parent` set), `CSA_DEPTH > 0`, or marker
/// I/O fails.  Caller is responsible for actually sleeping.
pub fn compute_cooldown_wait(
    project_root: &Path,
    cooldown_seconds: u64,
    session_arg: &Option<String>,
    parent: &Option<String>,
) -> Option<Duration> {
    if cooldown_seconds == 0 || session_arg.is_some() || parent.is_some() {
        return None;
    }
    let depth: u32 = std::env::var("CSA_DEPTH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if depth > 0 {
        return None;
    }
    let root = crate::get_session_root(project_root).ok()?;
    let last = read_cooldown_marker(&root).map(|m| m.completed_at);
    match evaluate_cooldown(last, cooldown_seconds, Utc::now()) {
        CooldownAction::Proceed => None,
        CooldownAction::Wait(d) => Some(d),
    }
}

// ---------------------------------------------------------------------------
// Convenience helpers
// ---------------------------------------------------------------------------

/// Best-effort marker write using `project_root` and `session_id`.
///
/// Resolves the sessions directory via [`crate::get_session_dir`], derives
/// the parent, and writes the marker.  All errors (resolution, I/O) are
/// logged via `tracing::warn` but never propagated.
pub fn write_cooldown_marker_for_project(
    project_root: &Path,
    session_id: &str,
    completed_at: DateTime<Utc>,
) {
    let session_dir = match crate::get_session_dir(project_root, session_id) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("cooldown marker: cannot resolve session dir: {e}");
            return;
        }
    };
    write_cooldown_marker_from_session_dir(&session_dir, session_id, completed_at);
}

/// Best-effort marker write using a session directory (resolves parent).
///
/// Silently swallows all errors (missing parent dir, I/O failures).
pub fn write_cooldown_marker_from_session_dir(
    session_dir: &Path,
    session_id: &str,
    completed_at: DateTime<Utc>,
) {
    if let Some(sessions_dir) = session_dir.parent()
        && let Err(e) = write_cooldown_marker(sessions_dir, session_id, completed_at)
    {
        tracing::warn!("cooldown marker write failed: {e}");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs_since_epoch: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs_since_epoch, 0).unwrap()
    }

    // -- evaluate_cooldown --------------------------------------------------

    #[test]
    fn test_evaluate_cooldown_expired() {
        // 120s cooldown, last completed 200s ago → Proceed
        let last = ts(1000);
        let now = ts(1200);
        assert_eq!(
            evaluate_cooldown(Some(last), 120, now),
            CooldownAction::Proceed
        );
    }

    #[test]
    fn test_evaluate_cooldown_not_expired() {
        // 120s cooldown, last completed 30s ago → Wait(~90s)
        let last = ts(1000);
        let now = ts(1030);
        let action = evaluate_cooldown(Some(last), 120, now);
        match action {
            CooldownAction::Wait(d) => {
                assert_eq!(d, Duration::from_secs(90));
            }
            other => panic!("expected Wait, got {other:?}"),
        }
    }

    #[test]
    fn test_evaluate_cooldown_disabled() {
        // cooldown_seconds == 0 → always Proceed
        let last = ts(1000);
        let now = ts(1001);
        assert_eq!(
            evaluate_cooldown(Some(last), 0, now),
            CooldownAction::Proceed
        );
    }

    #[test]
    fn test_evaluate_cooldown_no_last_completed() {
        // No previous completion → Proceed
        let now = ts(1000);
        assert_eq!(evaluate_cooldown(None, 120, now), CooldownAction::Proceed);
    }

    #[test]
    fn test_evaluate_cooldown_exact_boundary() {
        // elapsed == cooldown_seconds → Proceed (>=)
        let last = ts(1000);
        let now = ts(1120);
        assert_eq!(
            evaluate_cooldown(Some(last), 120, now),
            CooldownAction::Proceed
        );
    }

    // -- marker I/O ---------------------------------------------------------

    #[test]
    fn test_marker_write_then_read() {
        let dir = tempfile::tempdir().unwrap();
        let completed = ts(1_700_000_000);

        write_cooldown_marker(dir.path(), "01JTEST1234567890ABCDE", completed).unwrap();

        let marker = read_cooldown_marker(dir.path()).expect("should read back marker");
        assert_eq!(marker.session_id, "01JTEST1234567890ABCDE");
        assert_eq!(marker.completed_at, completed);
    }

    #[test]
    fn test_marker_read_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_cooldown_marker(dir.path()), None);
    }

    #[test]
    fn test_marker_read_corrupted_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MARKER_FILENAME);
        std::fs::write(&path, b"this is not valid TOML{{{").unwrap();

        // Should return None, not panic
        assert_eq!(read_cooldown_marker(dir.path()), None);
    }

    #[test]
    fn test_marker_sequential_writes() {
        let dir = tempfile::tempdir().unwrap();

        write_cooldown_marker(dir.path(), "SESSION_A", ts(1_700_000_000)).unwrap();
        write_cooldown_marker(dir.path(), "SESSION_B", ts(1_700_001_000)).unwrap();

        let marker = read_cooldown_marker(dir.path()).expect("should read latest marker");
        assert_eq!(marker.session_id, "SESSION_B");
        assert_eq!(marker.completed_at, ts(1_700_001_000));
    }

    // -- integration scenarios (debate-flagged) --------------------------------

    /// Scenario 1: Old session finishes later than new session (P0 debate finding).
    ///
    /// Session A has an older ULID but completes AFTER session B in wall-clock
    /// time. The marker uses last-writer-wins semantics: whichever session calls
    /// `write_cooldown_marker` last determines the marker content, regardless of
    /// the `completed_at` timestamp stored inside.
    #[test]
    fn test_integration_last_writer_wins_regardless_of_completed_at() {
        let dir = tempfile::tempdir().unwrap();

        // Session B completes first (wall-clock) with completed_at = t1
        let t1 = ts(1_700_000_100);
        write_cooldown_marker(dir.path(), "01JSESSION_B_NEWER_ULID", t1).unwrap();

        // Session A completes later (wall-clock) with completed_at = t2 > t1
        // Even though A's ULID is "older", it writes LAST → it wins.
        let t2 = ts(1_700_000_200);
        write_cooldown_marker(dir.path(), "01JSESSION_A_OLDER_ULID", t2).unwrap();

        let marker = read_cooldown_marker(dir.path()).expect("marker should exist");
        assert_eq!(marker.session_id, "01JSESSION_A_OLDER_ULID");
        assert_eq!(marker.completed_at, t2);

        // Cooldown evaluation uses A's completed_at (the last writer)
        let now = ts(1_700_000_210); // 10s after A completed
        let action = evaluate_cooldown(Some(marker.completed_at), 30, now);
        match action {
            CooldownAction::Wait(d) => assert_eq!(d, Duration::from_secs(20)),
            other => panic!("expected Wait(20s), got {other:?}"),
        }
    }

    /// Scenario 2: In-progress session (no result.toml) does not interfere.
    ///
    /// The cooldown system reads only the marker file, not the sessions directory.
    /// A newer session that is still running (no result.toml, no marker write)
    /// does not affect cooldown evaluation based on a previous marker.
    #[test]
    fn test_integration_running_session_does_not_affect_marker() {
        let dir = tempfile::tempdir().unwrap();

        // Session X completed and wrote a marker
        let completed_x = ts(1_700_000_000);
        write_cooldown_marker(dir.path(), "SESSION_X", completed_x).unwrap();

        // Session Y is "running" — its dir exists but no result.toml, no marker write.
        // We simulate by simply creating a subdirectory (as session management would).
        let session_y_dir = dir.path().join("SESSION_Y");
        std::fs::create_dir_all(&session_y_dir).unwrap();
        // No write_cooldown_marker for Y — it hasn't completed.

        // Reading the marker still returns X's data, unaffected by Y's existence.
        let marker = read_cooldown_marker(dir.path()).expect("marker should exist");
        assert_eq!(marker.session_id, "SESSION_X");
        assert_eq!(marker.completed_at, completed_x);

        // Cooldown evaluation works against X's completion time
        let now = ts(1_700_000_050); // 50s after X
        let action = evaluate_cooldown(Some(marker.completed_at), 120, now);
        match action {
            CooldownAction::Wait(d) => assert_eq!(d, Duration::from_secs(70)),
            other => panic!("expected Wait(70s), got {other:?}"),
        }
    }

    /// Scenario 3: Corrupted marker → graceful fallback to Proceed.
    ///
    /// NOTE: `test_marker_read_corrupted_file` already tests read returns None.
    /// `test_evaluate_cooldown_no_last_completed` already tests None → Proceed.
    /// This test verifies the end-to-end path: corrupted file → read → evaluate → Proceed.
    #[test]
    fn test_integration_corrupted_marker_falls_through_to_proceed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MARKER_FILENAME);
        std::fs::write(&path, b"\x00\x01\x02 garbage {{not toml").unwrap();

        // End-to-end: read corrupted → None → evaluate → Proceed
        let marker = read_cooldown_marker(dir.path());
        assert_eq!(marker, None);

        let now = Utc::now();
        let action = evaluate_cooldown(marker.map(|m| m.completed_at), 120, now);
        assert_eq!(action, CooldownAction::Proceed);
    }

    /// Scenario 4: cooldown_seconds=0 bypasses cooldown with fresh marker.
    ///
    /// NOTE: `test_evaluate_cooldown_disabled` covers the pure function.
    /// This test adds the I/O path: write a fresh marker, read it back,
    /// then verify cooldown_seconds=0 still returns Proceed.
    #[test]
    fn test_integration_zero_cooldown_bypasses_with_fresh_marker() {
        let dir = tempfile::tempdir().unwrap();

        // Write a marker with completed_at = now (maximally fresh)
        let now = Utc::now();
        write_cooldown_marker(dir.path(), "FRESH_SESSION", now).unwrap();

        let marker = read_cooldown_marker(dir.path()).expect("marker should exist");
        assert_eq!(marker.session_id, "FRESH_SESSION");

        // Even though marker is fresh, cooldown_seconds=0 → Proceed
        let action = evaluate_cooldown(Some(marker.completed_at), 0, now);
        assert_eq!(action, CooldownAction::Proceed);
    }

    /// Scenario 5: Sequential writes (10x) don't corrupt — atomic rename semantics.
    ///
    /// NOTE: `test_marker_sequential_writes` covers 2 writes. This test scales
    /// to 10 writes and verifies no corruption occurs at any point.
    #[test]
    fn test_integration_sequential_writes_no_corruption() {
        let dir = tempfile::tempdir().unwrap();

        for i in 0..10 {
            let session_id = format!("SESSION_{i:02}");
            let completed_at = ts(1_700_000_000 + i * 100);
            write_cooldown_marker(dir.path(), &session_id, completed_at).unwrap();

            // Verify after each write — no partial/corrupt state
            let marker = read_cooldown_marker(dir.path()).expect("marker should be valid");
            assert_eq!(marker.session_id, session_id);
            assert_eq!(marker.completed_at, completed_at);
        }

        // Final read: must be the last writer
        let marker = read_cooldown_marker(dir.path()).expect("final marker should exist");
        assert_eq!(marker.session_id, "SESSION_09");
        assert_eq!(marker.completed_at, ts(1_700_000_900));
    }
}
