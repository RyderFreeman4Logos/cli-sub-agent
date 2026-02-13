//! Structured session execution result.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const RESULT_FILE_NAME: &str = "result.toml";

/// Structured result of a session execution.
/// Written to `sessions/{id}/result.toml` after each tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResult {
    /// Execution status: "success", "failure", "timeout", "signal"
    pub status: String,
    /// Tool exit code
    pub exit_code: i32,
    /// Brief summary of what happened (last meaningful output line, max 200 chars)
    pub summary: String,
    /// Tool that was executed
    pub tool: String,
    /// When execution started
    pub started_at: DateTime<Utc>,
    /// When execution completed
    pub completed_at: DateTime<Utc>,
    /// List of artifact paths relative to session dir (e.g., "output/diff.patch")
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
}

impl SessionResult {
    /// Derive status string from exit code
    pub fn status_from_exit_code(exit_code: i32) -> String {
        match exit_code {
            0 => "success".to_string(),
            137 | 143 => "signal".to_string(), // SIGKILL / SIGTERM
            _ => "failure".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    // ── Serialization round-trip ───────────────────────────────────

    #[test]
    fn test_session_result_toml_roundtrip() {
        let now = Utc::now();
        let result = SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "All tests passed".to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            artifacts: vec!["output/diff.patch".to_string()],
        };

        let toml_str = toml::to_string_pretty(&result).expect("Serialize should succeed");
        let loaded: SessionResult = toml::from_str(&toml_str).expect("Deserialize should succeed");

        assert_eq!(loaded.status, "success");
        assert_eq!(loaded.exit_code, 0);
        assert_eq!(loaded.summary, "All tests passed");
        assert_eq!(loaded.tool, "codex");
        assert_eq!(loaded.artifacts.len(), 1);
        assert_eq!(loaded.artifacts[0], "output/diff.patch");
    }

    #[test]
    fn test_session_result_empty_artifacts_omitted() {
        let now = Utc::now();
        let result = SessionResult {
            status: "failure".to_string(),
            exit_code: 1,
            summary: "Build failed".to_string(),
            tool: "gemini-cli".to_string(),
            started_at: now,
            completed_at: now,
            artifacts: vec![],
        };

        let toml_str = toml::to_string_pretty(&result).expect("Serialize should succeed");
        // skip_serializing_if = "Vec::is_empty" should omit the field
        assert!(
            !toml_str.contains("artifacts"),
            "Empty artifacts should be omitted from serialization"
        );

        // Deserialize back: missing artifacts field should default to empty vec
        let loaded: SessionResult = toml::from_str(&toml_str).expect("Deserialize should succeed");
        assert!(loaded.artifacts.is_empty());
    }

    // ── status_from_exit_code ──────────────────────────────────────

    #[test]
    fn test_status_from_exit_code_success() {
        assert_eq!(SessionResult::status_from_exit_code(0), "success");
    }

    #[test]
    fn test_status_from_exit_code_failure() {
        assert_eq!(SessionResult::status_from_exit_code(1), "failure");
        assert_eq!(SessionResult::status_from_exit_code(2), "failure");
        assert_eq!(SessionResult::status_from_exit_code(127), "failure");
    }

    #[test]
    fn test_status_from_exit_code_signal() {
        assert_eq!(SessionResult::status_from_exit_code(137), "signal"); // SIGKILL
        assert_eq!(SessionResult::status_from_exit_code(143), "signal"); // SIGTERM
    }

    #[test]
    fn test_status_from_exit_code_negative() {
        // Negative exit codes should be treated as failure
        assert_eq!(SessionResult::status_from_exit_code(-1), "failure");
    }

    // ── File I/O round-trip ────────────────────────────────────────

    #[test]
    fn test_session_result_file_roundtrip() {
        let tmp = tempfile::tempdir().expect("Failed to create temp dir");
        let path = tmp.path().join(RESULT_FILE_NAME);

        let now = Utc::now();
        let result = SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "Done".to_string(),
            tool: "opencode".to_string(),
            started_at: now,
            completed_at: now,
            artifacts: vec!["output/a.txt".to_string(), "output/b.txt".to_string()],
        };

        let contents = toml::to_string_pretty(&result).unwrap();
        std::fs::write(&path, &contents).expect("Write should succeed");

        let read_back = std::fs::read_to_string(&path).expect("Read should succeed");
        let loaded: SessionResult = toml::from_str(&read_back).expect("Parse should succeed");

        assert_eq!(loaded.status, result.status);
        assert_eq!(loaded.exit_code, result.exit_code);
        assert_eq!(loaded.artifacts.len(), 2);
    }
}
