//! Session directory scanner.
//!
//! Reads `result.toml` and `state.toml` from each session directory under
//! `{state_dir}/{project_key}/sessions/` and extracts lightweight summaries.
//! Uses `toml::Value` for deserialization to avoid coupling to csa-session's
//! internal types (which may have private fields or custom deserializers).

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Lightweight summary of a single session, extracted from result.toml + state.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub status: String,
    pub exit_code: i32,
    pub tool: Option<String>,
    pub tier_name: Option<String>,
    pub total_input_tokens: Option<u64>,
    pub total_output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
}

/// Scan session directories under `{state_dir}/{project_key}/sessions/`.
///
/// Returns summaries for sessions created within the last `days_back` days.
/// Missing or corrupt TOML files produce summaries with Unknown status rather than errors.
pub fn scan_sessions(
    state_dir: &Path,
    project_key: &str,
    days_back: u32,
) -> Result<Vec<SessionSummary>> {
    let sessions_dir = state_dir.join(project_key).join("sessions");
    if !sessions_dir.is_dir() {
        return Ok(Vec::new());
    }

    let cutoff = chrono::Utc::now() - chrono::Duration::days(i64::from(days_back));

    let mut summaries = Vec::new();
    let entries = std::fs::read_dir(&sessions_dir)?;

    for entry in entries {
        let entry = entry?;
        let dir_path = entry.path();

        // Only process directories
        if !dir_path.is_dir() {
            continue;
        }

        // Reject symlinks: canonicalize and verify still under state_dir
        let canonical = match dir_path.canonicalize() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let canonical_state = match state_dir.canonicalize() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !canonical.starts_with(&canonical_state) {
            tracing::warn!(
                path = %dir_path.display(),
                "session dir escapes state_dir via symlink, skipping"
            );
            continue;
        }

        let session_id = match entry.file_name().to_str() {
            Some(name) => name.to_string(),
            None => continue,
        };

        // Filter by ULID timestamp (first 10 chars encode time)
        if !is_within_timeframe(&session_id, &cutoff) {
            continue;
        }

        let summary = read_session_summary(&dir_path, &session_id);
        summaries.push(summary);
    }

    // Sort by session_id (ULID = chronological)
    summaries.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    Ok(summaries)
}

/// Check if a ULID session ID was created after the cutoff time.
fn is_within_timeframe(session_id: &str, cutoff: &chrono::DateTime<chrono::Utc>) -> bool {
    // ULID is 26 chars, Crockford Base32. First 10 chars = millisecond timestamp.
    if session_id.len() < 26 {
        return false;
    }

    match ulid::Ulid::from_string(session_id) {
        Ok(ulid) => {
            let ts_ms = ulid.timestamp_ms();
            let session_time = chrono::DateTime::from_timestamp_millis(ts_ms as i64);
            match session_time {
                Some(t) => t >= *cutoff,
                None => false,
            }
        }
        Err(_) => false, // Not a valid ULID, skip
    }
}

/// Read result.toml and state.toml from a session directory, returning a summary.
/// On any read/parse error, returns a summary with Unknown status.
fn read_session_summary(session_dir: &Path, session_id: &str) -> SessionSummary {
    let mut summary = SessionSummary {
        session_id: session_id.to_string(),
        status: "unknown".to_string(),
        exit_code: -1,
        tool: None,
        tier_name: None,
        total_input_tokens: None,
        total_output_tokens: None,
        total_tokens: None,
        estimated_cost_usd: None,
    };

    // Read result.toml
    let result_path = session_dir.join("result.toml");
    if let Ok(content) = std::fs::read_to_string(&result_path)
        && let Ok(val) = toml::from_str::<toml::Value>(content.trim())
    {
        if let Some(status) = val.get("status").and_then(|v| v.as_str()) {
            summary.status = status.to_string();
        }
        if let Some(code) = val.get("exit_code").and_then(|v| v.as_integer()) {
            summary.exit_code = code as i32;
        }
        if let Some(tool) = val.get("tool").and_then(|v| v.as_str()) {
            summary.tool = Some(tool.to_string());
        }
    }

    // Read state.toml for token usage and tier_name
    let state_path = session_dir.join("state.toml");
    if let Ok(content) = std::fs::read_to_string(&state_path)
        && let Ok(val) = toml::from_str::<toml::Value>(content.trim())
    {
        // total_token_usage table
        if let Some(usage) = val.get("total_token_usage").and_then(|v| v.as_table()) {
            summary.total_input_tokens = usage
                .get("input_tokens")
                .and_then(|v| v.as_integer())
                .map(|v| v as u64);
            summary.total_output_tokens = usage
                .get("output_tokens")
                .and_then(|v| v.as_integer())
                .map(|v| v as u64);
            summary.total_tokens = usage
                .get("total_tokens")
                .and_then(|v| v.as_integer())
                .map(|v| v as u64);
            summary.estimated_cost_usd = usage.get("estimated_cost_usd").and_then(|v| v.as_float());
        }

        // task_context.tier_name
        if let Some(ctx) = val.get("task_context").and_then(|v| v.as_table())
            && let Some(tier) = ctx.get("tier_name").and_then(|v| v.as_str())
        {
            summary.tier_name = Some(tier.to_string());
        }
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a session directory with optional result.toml and state.toml.
    fn create_session_dir(
        base: &Path,
        session_id: &str,
        result_toml: Option<&str>,
        state_toml: Option<&str>,
    ) {
        let session_dir = base.join("sessions").join(session_id);
        fs::create_dir_all(&session_dir).unwrap();
        if let Some(content) = result_toml {
            fs::write(session_dir.join("result.toml"), content).unwrap();
        }
        if let Some(content) = state_toml {
            fs::write(session_dir.join("state.toml"), content).unwrap();
        }
    }

    /// Generate a ULID string for "now" so it passes the days_back filter.
    fn fresh_ulid() -> String {
        ulid::Ulid::new().to_string()
    }

    /// Generate a ULID string for a time far in the past (filtered out).
    fn old_ulid() -> String {
        let old_ms = 0u64; // epoch
        ulid::Ulid::from_parts(old_ms, 0).to_string()
    }

    #[test]
    fn test_scan_sessions_normal() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("test-project");
        fs::create_dir_all(&project_dir).unwrap();

        let sid = fresh_ulid();
        create_session_dir(
            &project_dir,
            &sid,
            Some(
                r#"
status = "success"
exit_code = 0
summary = "done"
tool = "claude-code"
started_at = "2025-01-01T00:00:00Z"
completed_at = "2025-01-01T00:05:00Z"
"#,
            ),
            Some(
                r#"
meta_session_id = "TEST"
project_path = "/tmp"
created_at = "2025-01-01T00:00:00Z"
last_accessed = "2025-01-01T00:05:00Z"

[total_token_usage]
input_tokens = 1000
output_tokens = 500
total_tokens = 1500
estimated_cost_usd = 0.05

[task_context]
tier_name = "tier-1"
"#,
            ),
        );

        let results = scan_sessions(tmp.path(), "test-project", 30).unwrap();
        assert_eq!(results.len(), 1);

        let s = &results[0];
        assert_eq!(s.session_id, sid);
        assert_eq!(s.status, "success");
        assert_eq!(s.exit_code, 0);
        assert_eq!(s.tool.as_deref(), Some("claude-code"));
        assert_eq!(s.tier_name.as_deref(), Some("tier-1"));
        assert_eq!(s.total_input_tokens, Some(1000));
        assert_eq!(s.total_output_tokens, Some(500));
        assert_eq!(s.total_tokens, Some(1500));
        assert!((s.estimated_cost_usd.unwrap() - 0.05).abs() < 0.001);
    }

    #[test]
    fn test_scan_sessions_missing_result() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("test-project");
        fs::create_dir_all(&project_dir).unwrap();

        let sid = fresh_ulid();
        create_session_dir(&project_dir, &sid, None, None);

        let results = scan_sessions(tmp.path(), "test-project", 30).unwrap();
        assert_eq!(results.len(), 1);

        let s = &results[0];
        assert_eq!(s.status, "unknown");
        assert_eq!(s.exit_code, -1);
        assert!(s.tool.is_none());
    }

    #[test]
    fn test_scan_sessions_corrupt_state() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("test-project");
        fs::create_dir_all(&project_dir).unwrap();

        let sid = fresh_ulid();
        create_session_dir(
            &project_dir,
            &sid,
            Some(
                r#"
status = "failure"
exit_code = 1
summary = "bad"
tool = "gemini-cli"
started_at = "2025-01-01T00:00:00Z"
completed_at = "2025-01-01T00:05:00Z"
"#,
            ),
            Some("this is not valid TOML {{{"),
        );

        let results = scan_sessions(tmp.path(), "test-project", 30).unwrap();
        assert_eq!(results.len(), 1);

        let s = &results[0];
        assert_eq!(s.status, "failure");
        assert_eq!(s.exit_code, 1);
        assert_eq!(s.tool.as_deref(), Some("gemini-cli"));
        // Token fields should be None due to corrupt state.toml
        assert!(s.total_tokens.is_none());
        assert!(s.tier_name.is_none());
    }

    #[test]
    fn test_scan_sessions_filters_old() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("test-project");
        fs::create_dir_all(&project_dir).unwrap();

        let old_sid = old_ulid();
        let fresh_sid = fresh_ulid();
        create_session_dir(
            &project_dir,
            &old_sid,
            Some(
                r#"
status = "success"
exit_code = 0
summary = "old"
tool = "codex"
started_at = "1970-01-01T00:00:00Z"
completed_at = "1970-01-01T00:00:01Z"
"#,
            ),
            None,
        );
        create_session_dir(
            &project_dir,
            &fresh_sid,
            Some(
                r#"
status = "success"
exit_code = 0
summary = "fresh"
tool = "codex"
started_at = "2025-01-01T00:00:00Z"
completed_at = "2025-01-01T00:05:00Z"
"#,
            ),
            None,
        );

        let results = scan_sessions(tmp.path(), "test-project", 7).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, fresh_sid);
    }

    #[test]
    fn test_scan_sessions_nonexistent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let results = scan_sessions(tmp.path(), "nonexistent-project", 30).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_toml_parse_result() {
        let content = r#"status = "success"
exit_code = 0
summary = "done"
tool = "claude-code"
started_at = "2025-01-01T00:00:00Z"
completed_at = "2025-01-01T00:05:00Z"
"#;
        let val = toml::from_str::<toml::Value>(content);
        eprintln!("parse result: {:?}", val);
        assert!(val.is_ok(), "failed to parse: {:?}", val.err());
        let val = val.unwrap();
        assert_eq!(val.get("status").and_then(|v| v.as_str()), Some("success"));
    }
}
