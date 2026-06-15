//! Structured session execution result.

use crate::kill_diagnostics::KillDiagnosticReport;
use crate::large_diff_warning::LargeDiffWarningReport;
use chrono::{DateTime, Utc};
use csa_core::types::FallbackAttempt;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use toml::Value as TomlValue;

pub const RESULT_FILE_NAME: &str = "result.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionArtifact {
    /// Artifact path relative to session dir (e.g., "output/acp-events.jsonl")
    pub path: String,
    /// True when the artifact was discovered for display/diagnostics only.
    ///
    /// Display-only manager result artifacts must not drive manager sidecar
    /// overlay or write-target selection.
    #[serde(default, skip_serializing_if = "is_false")]
    pub display_only: bool,
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
            display_only: false,
            line_count: None,
            size_bytes: None,
        }
    }

    pub fn display_only(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            display_only: true,
            line_count: None,
            size_bytes: None,
        }
    }

    pub fn with_stats(path: impl Into<String>, line_count: u64, size_bytes: u64) -> Self {
        Self {
            path: path.into(),
            display_only: false,
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

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !value
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

/// Summary of dirty worktree state left by a writer session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UncommittedChanges {
    /// Number of paths reported by `git status --porcelain`.
    pub file_count: usize,
    /// Best-effort inserted line count from `git diff --numstat HEAD`.
    pub insertions: u64,
    /// Best-effort deleted line count from `git diff --numstat HEAD`.
    pub deletions: u64,
    /// Approximate token count of the changed diff payload.
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub approx_diff_tokens: usize,
    /// First changed paths, capped to keep `result.toml` compact.
    pub files: Vec<String>,
    /// Number of paths omitted from `files` due to the cap.
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub truncated: usize,
}

impl UncommittedChanges {
    pub fn changed_lines(&self) -> u64 {
        self.insertions.saturating_add(self.deletions)
    }
}

/// Structured result of a session execution.
/// Written to `sessions/{id}/result.toml` after each tool invocation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// Best-effort signal-exit diagnostic hint, not a definitive kill cause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kill_hint: Option<String>,
    /// Structured details for concrete kill sources known to CSA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kill_diagnostics: Option<KillDiagnosticReport>,
    /// Last known work item when the signal diagnostic was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_item: Option<String>,
    /// Ordered list of tools skipped due to quota/rate-limit failover before the final tool ran.
    /// `None` when no failover occurred; non-empty only when `csa run` cycled through alternatives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_chain: Option<Vec<FallbackAttempt>>,
    /// Whether the session failed due to a post-exec gate timeout.
    /// Only set when post-exec gate times out; false otherwise.
    #[serde(default, skip_serializing_if = "is_false")]
    pub gate_timeout: bool,
    /// Non-fatal warnings attached to a `success` status. Populated when the
    /// effective-outcome classifier downgrades an incidental nonzero exit on a
    /// completed turn to success (#161): the session achieved its purpose, but
    /// a hook or in-turn command exited nonzero. Empty for clean sessions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// The raw tool-process exit code, preserved as a diagnostic when it differs
    /// from the effective `exit_code` (i.e. when an incidental nonzero exit was
    /// downgraded to success). `None` when the raw exit matched the effective
    /// status, so existing clean envelopes are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_process_exit_code: Option<i32>,
    /// Dirty worktree state observed when a non-SA writer session ended without
    /// committing. Omitted for clean sessions and read-only session kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncommitted_changes: Option<UncommittedChanges>,
    /// Structured large-diff warning data derived from `uncommitted_changes`.
    /// Omitted when the dirty surface stays below the configured warning thresholds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub large_diff_warning: Option<LargeDiffWarningReport>,
    /// Structured detail of a failed post-exec verification gate (#1726).
    /// Present ONLY when the gate (e.g. `just pre-commit`) failed, so an SA
    /// orchestrator can diagnose the failing step/test from `result.toml`
    /// without reading the raw transcript. The bounded tail lives here; the full
    /// gate output is written to `output/gate-failure.log`. `serde(default)` +
    /// `skip_serializing_if` keeps the table absent on success and lets
    /// pre-existing `result.toml` files (without it) still deserialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_exec_gate: Option<crate::post_exec_gate_report::PostExecGateReport>,
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
#[path = "result_kill_diagnostics_tests.rs"]
mod kill_diagnostics_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    // ── Serialization round-trip ───────────────────────────────────

    #[test]
    fn test_session_result_toml_roundtrip() {
        let now = Utc::now();
        let result = SessionResult {
            post_exec_gate: None,
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
            kill_hint: Some("memory_pressure".to_string()),
            last_item: Some("running tests".to_string()),
            fallback_chain: None,
            ..Default::default()
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
        assert_eq!(loaded.kill_hint.as_deref(), Some("memory_pressure"));
        assert_eq!(loaded.last_item.as_deref(), Some("running tests"));
    }

    #[test]
    fn test_session_result_empty_optional_fields_omitted() {
        let now = Utc::now();
        let result = SessionResult {
            post_exec_gate: None,
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
            ..Default::default()
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
        assert!(
            !toml_str.contains("uncommitted_changes"),
            "Clean sessions should omit uncommitted_changes"
        );
        assert!(
            !toml_str.contains("kill_hint"),
            "Missing kill hint should be omitted from serialization"
        );
        assert!(
            !toml_str.contains("kill_diagnostics"),
            "Missing kill diagnostics should be omitted from serialization"
        );
        assert!(
            !toml_str.contains("last_item"),
            "Missing last_item should be omitted from serialization"
        );

        let loaded: SessionResult = toml::from_str(&toml_str).expect("Deserialize should succeed");
        assert!(loaded.artifacts.is_empty());
        assert_eq!(loaded.events_count, 0);
        assert!(loaded.uncommitted_changes.is_none());
    }

    #[test]
    fn test_session_result_uncommitted_changes_roundtrip() {
        let now = Utc::now();
        let result = SessionResult {
            post_exec_gate: None,
            status: "success".to_string(),
            exit_code: 0,
            summary: "Done".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: vec![],
            peak_memory_mb: None,
            kill_hint: None,
            kill_diagnostics: None,
            last_item: None,
            fallback_chain: None,
            gate_timeout: false,
            warnings: Vec::new(),
            raw_process_exit_code: None,
            uncommitted_changes: Some(UncommittedChanges {
                file_count: 7,
                insertions: 240,
                deletions: 12,
                approx_diff_tokens: 1_024,
                files: vec!["src/lib.rs".to_string()],
                truncated: 6,
            }),
            large_diff_warning: Some(LargeDiffWarningReport {
                changed_files: 7,
                changed_lines: 252,
                approx_diff_tokens: 1_024,
            }),
            manager_fields: Default::default(),
        };

        let toml_str = toml::to_string_pretty(&result).expect("Serialize should succeed");
        assert!(toml_str.contains("[uncommitted_changes]"));

        let loaded: SessionResult = toml::from_str(&toml_str).expect("Deserialize should succeed");
        let changes = loaded
            .uncommitted_changes
            .expect("uncommitted_changes should roundtrip");
        assert_eq!(changes.file_count, 7);
        assert_eq!(changes.insertions, 240);
        assert_eq!(changes.deletions, 12);
        assert_eq!(changes.changed_lines(), 252);
        assert_eq!(changes.approx_diff_tokens, 1_024);
        assert_eq!(changes.truncated, 6);
        let warning = loaded
            .large_diff_warning
            .expect("large_diff_warning should roundtrip");
        assert_eq!(warning.changed_files, 7);
        assert_eq!(warning.changed_lines, 252);
        assert_eq!(warning.approx_diff_tokens, 1_024);
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

    #[test]
    fn test_result_without_post_exec_gate_deserializes_to_none() {
        // Backward compat (#1726): pre-existing result.toml files have no
        // [post_exec_gate] table; they must still deserialize, with the field None.
        let raw = r#"
status = "success"
exit_code = 0
summary = "ok"
tool = "codex"
started_at = "2026-01-01T00:00:00Z"
completed_at = "2026-01-01T00:00:00Z"
"#;
        let loaded: SessionResult = toml::from_str(raw).expect("Deserialize should succeed");
        assert!(loaded.post_exec_gate.is_none());
    }

    #[test]
    fn test_successful_result_omits_post_exec_gate_table() {
        // A successful (post_exec_gate = None) result must serialize WITHOUT a
        // [post_exec_gate] table, so successful sessions emit no spurious table.
        let now = Utc::now();
        let result = SessionResult {
            post_exec_gate: None,
            status: "success".to_string(),
            exit_code: 0,
            summary: "ok".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
            ..Default::default()
        };
        let toml_str = toml::to_string_pretty(&result).expect("serialize");
        assert!(
            !toml_str.contains("post_exec_gate"),
            "successful result must not serialize a [post_exec_gate] table: {toml_str}"
        );
    }

    #[test]
    fn test_result_with_post_exec_gate_table_roundtrips() {
        // A failed-gate result round-trips its [post_exec_gate] table intact.
        let now = Utc::now();
        let report = crate::post_exec_gate_report::PostExecGateReport::from_redacted_gate_output(
            "just pre-commit",
            100,
            "FAIL [   0.005s] pkg::a\nerror: Recipe `test` failed on line 1 with exit code 100",
        );
        let result = SessionResult {
            post_exec_gate: Some(report.clone()),
            status: "failure".to_string(),
            exit_code: 1,
            summary: "POST-EXEC GATE FAILED (exit=100, step=just test)".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
            ..Default::default()
        };
        let toml_str = toml::to_string_pretty(&result).expect("serialize");
        let loaded: SessionResult = toml::from_str(&toml_str).expect("Deserialize should succeed");
        assert_eq!(loaded.post_exec_gate, Some(report));
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
            post_exec_gate: None,
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
            ..Default::default()
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
