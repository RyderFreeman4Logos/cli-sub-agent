//! Structured session execution result.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use toml::Value as TomlValue;

pub const RESULT_FILE_NAME: &str = "result.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionArtifact {
    /// Artifact path relative to session dir (e.g., "output/acp-events.jsonl")
    pub path: String,
    /// Optional number of lines (used by transcript JSONL artifacts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_count: Option<u64>,
    /// Optional file size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

impl SessionArtifact {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            line_count: None,
            size_bytes: None,
        }
    }

    pub fn with_stats(path: impl Into<String>, line_count: u64, size_bytes: u64) -> Self {
        Self {
            path: path.into(),
            line_count: Some(line_count),
            size_bytes: Some(size_bytes),
        }
    }
}

impl fmt::Display for SessionArtifact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.line_count, self.size_bytes) {
            (Some(lines), Some(bytes)) => {
                write!(f, "{} (lines={}, bytes={})", self.path, lines, bytes)
            }
            (Some(lines), None) => write!(f, "{} (lines={})", self.path, lines),
            (None, Some(bytes)) => write!(f, "{} (bytes={})", self.path, bytes),
            (None, None) => f.write_str(&self.path),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SessionArtifactCompat {
    Path(String),
    Detailed(SessionArtifact),
}

fn deserialize_artifacts<'de, D>(deserializer: D) -> Result<Vec<SessionArtifact>, D::Error>
where
    D: Deserializer<'de>,
{
    let compat = Vec::<SessionArtifactCompat>::deserialize(deserializer)?;
    Ok(compat
        .into_iter()
        .map(|entry| match entry {
            SessionArtifactCompat::Path(path) => SessionArtifact::new(path),
            SessionArtifactCompat::Detailed(detailed) => detailed,
        })
        .collect())
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionManagerFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TomlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<TomlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<TomlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<TomlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<TomlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clarification: Option<TomlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<TomlValue>,
}

impl SessionManagerFields {
    pub fn from_sidecar(sidecar: &TomlValue) -> Self {
        Self {
            result: sidecar.get("result").cloned(),
            report: sidecar.get("report").cloned(),
            timing: sidecar.get("timing").cloned(),
            tool: sidecar.get("tool").cloned(),
            review: sidecar.get("review").cloned(),
            clarification: sidecar.get("clarification").cloned(),
            artifacts: sidecar.get("artifacts").cloned(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.result.is_none()
            && self.report.is_none()
            && self.timing.is_none()
            && self.tool.is_none()
            && self.review.is_none()
            && self.clarification.is_none()
            && self.artifacts.is_none()
    }

    pub fn as_sidecar(&self) -> Option<TomlValue> {
        if self.is_empty() {
            return None;
        }

        let mut table = toml::map::Map::<String, TomlValue>::new();
        if let Some(result) = self.result.as_ref() {
            table.insert("result".to_string(), result.clone());
        }
        if let Some(report) = self.report.as_ref() {
            table.insert("report".to_string(), report.clone());
        }
        if let Some(timing) = self.timing.as_ref() {
            table.insert("timing".to_string(), timing.clone());
        }
        if let Some(tool) = self.tool.as_ref() {
            table.insert("tool".to_string(), tool.clone());
        }
        if let Some(review) = self.review.as_ref() {
            table.insert("review".to_string(), review.clone());
        }
        if let Some(clarification) = self.clarification.as_ref() {
            table.insert("clarification".to_string(), clarification.clone());
        }
        if let Some(artifacts) = self.artifacts.as_ref() {
            table.insert("artifacts".to_string(), artifacts.clone());
        }
        Some(TomlValue::Table(table))
    }
}

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
    /// First tool selected before runtime fallback, when fallback occurred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_tool: Option<String>,
    /// Tool that ultimately produced this result, when different from original_tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_tool: Option<String>,
    /// Machine-readable reason for runtime fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    /// When execution started
    pub started_at: DateTime<Utc>,
    /// When execution completed
    pub completed_at: DateTime<Utc>,
    /// Number of ACP events observed from transport.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub events_count: u64,
    /// List of artifact metadata relative to session dir.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_artifacts"
    )]
    pub artifacts: Vec<SessionArtifact>,
    /// Peak memory usage in MB observed during execution (from cgroup memory.peak).
    /// `None` when cgroup monitoring is unavailable or the scope was already removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_memory_mb: Option<u64>,
    /// Manager-facing data loaded from `output/result.toml` sidecars at read time.
    /// This is intentionally read-only metadata and is never serialized back into
    /// the runtime `result.toml` envelope.
    #[serde(skip_serializing, skip_deserializing, default)]
    pub manager_fields: SessionManagerFields,
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
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 4,
            artifacts: vec![SessionArtifact::new("output/diff.patch")],
            peak_memory_mb: None,
            manager_fields: Default::default(),
        };

        let toml_str = toml::to_string_pretty(&result).expect("Serialize should succeed");
        let loaded: SessionResult = toml::from_str(&toml_str).expect("Deserialize should succeed");

        assert_eq!(loaded.status, "success");
        assert_eq!(loaded.exit_code, 0);
        assert_eq!(loaded.summary, "All tests passed");
        assert_eq!(loaded.tool, "codex");
        assert_eq!(loaded.events_count, 4);
        assert_eq!(loaded.artifacts.len(), 1);
        assert_eq!(loaded.artifacts[0].path, "output/diff.patch");
    }

    #[test]
    fn test_session_result_empty_optional_fields_omitted() {
        let now = Utc::now();
        let result = SessionResult {
            status: "failure".to_string(),
            exit_code: 1,
            summary: "Build failed".to_string(),
            tool: "gemini-cli".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: vec![],
            peak_memory_mb: None,
            manager_fields: Default::default(),
        };

        let toml_str = toml::to_string_pretty(&result).expect("Serialize should succeed");
        assert!(
            !toml_str.contains("artifacts"),
            "Empty artifacts should be omitted from serialization"
        );
        assert!(
            !toml_str.contains("events_count"),
            "Zero events_count should be omitted from serialization"
        );

        let loaded: SessionResult = toml::from_str(&toml_str).expect("Deserialize should succeed");
        assert!(loaded.artifacts.is_empty());
        assert_eq!(loaded.events_count, 0);
    }

    #[test]
    fn test_session_result_artifacts_support_legacy_path_strings() {
        let raw = r#"
status = "success"
exit_code = 0
summary = "ok"
tool = "codex"
started_at = "2026-01-01T00:00:00Z"
completed_at = "2026-01-01T00:00:00Z"
artifacts = ["output/a.txt", "output/b.txt"]
"#;
        let loaded: SessionResult = toml::from_str(raw).expect("Deserialize should succeed");
        assert_eq!(loaded.artifacts.len(), 2);
        assert_eq!(loaded.artifacts[0].path, "output/a.txt");
        assert_eq!(loaded.artifacts[1].path, "output/b.txt");
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
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 2,
            artifacts: vec![
                SessionArtifact::new("output/a.txt"),
                SessionArtifact::with_stats("output/acp-events.jsonl", 10, 256),
            ],
            peak_memory_mb: None,
            manager_fields: Default::default(),
        };

        let contents = toml::to_string_pretty(&result).unwrap();
        std::fs::write(&path, &contents).expect("Write should succeed");

        let read_back = std::fs::read_to_string(&path).expect("Read should succeed");
        let loaded: SessionResult = toml::from_str(&read_back).expect("Parse should succeed");

        assert_eq!(loaded.status, result.status);
        assert_eq!(loaded.exit_code, result.exit_code);
        assert_eq!(loaded.events_count, 2);
        assert_eq!(loaded.artifacts.len(), 2);
    }
}
