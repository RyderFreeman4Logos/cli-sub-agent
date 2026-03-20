//! Handoff artifact generation for session knowledge transfer.
//!
//! Writes `handoff.toml` to the session directory after tool execution,
//! capturing structured context-transfer fields from the return packet.

use std::fs;
use std::path::Path;

use serde::Serialize;
use tracing::{info, warn};

use csa_session::MetaSessionState;

/// Structured handoff artifact persisted as `handoff.toml` in the session directory.
///
/// Captures the session's outcome and context-transfer fields so that subsequent
/// sessions (fork-call, manual resume, or parent orchestrators) can bootstrap
/// with full knowledge of what happened, what worked, and what to do next.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct HandoffArtifact {
    /// Session metadata.
    pub session: HandoffSessionMeta,
    /// Structured context-transfer fields extracted from the return packet.
    pub handoff: HandoffFields,
}

/// Session-level metadata included in the handoff artifact.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct HandoffSessionMeta {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub exit_code: i32,
    pub tool: String,
    pub duration_seconds: i64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Context-transfer fields sourced from the ReturnPacket handoff extensions.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct HandoffFields {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tried_and_worked: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tried_and_failed: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub next_steps: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub key_decisions: Vec<String>,
}

/// Write `handoff.toml` to the session directory.
///
/// Reads the persisted `return-packet` structured output section (if any) to
/// extract handoff fields, then combines them with session metadata into a
/// single TOML artifact.
///
/// Best-effort: logs a warning and returns quietly on any error.
pub(crate) fn write_handoff_artifact(
    session_dir: &Path,
    session: &MetaSessionState,
    result: &csa_process::ExecutionResult,
    tool_name: &str,
    execution_start_time: chrono::DateTime<chrono::Utc>,
) {
    if !session_dir.exists() {
        warn!(
            session_dir = %session_dir.display(),
            "Session directory does not exist; skipping handoff.toml"
        );
        return;
    }

    // Try to read handoff fields from the persisted return-packet section.
    let handoff_fields =
        match csa_session::read_section(session_dir, csa_session::RETURN_PACKET_SECTION_ID) {
            Ok(Some(content)) => match csa_session::parse_return_packet(&content) {
                Ok(packet) => HandoffFields {
                    tried_and_worked: packet.tried_and_worked,
                    tried_and_failed: packet.tried_and_failed,
                    next_steps: packet.next_steps,
                    key_decisions: packet.key_decisions,
                },
                Err(e) => {
                    warn!("Failed to parse return packet for handoff: {e}");
                    HandoffFields::default()
                }
            },
            Ok(None) => HandoffFields::default(),
            Err(e) => {
                warn!("Failed to read return-packet section for handoff: {e}");
                HandoffFields::default()
            }
        };

    let now = chrono::Utc::now();
    let duration_seconds = (now - execution_start_time).num_seconds();

    let artifact = HandoffArtifact {
        session: HandoffSessionMeta {
            session_id: session.meta_session_id.clone(),
            branch: session.branch.clone(),
            description: session.description.clone(),
            exit_code: result.exit_code,
            tool: tool_name.to_string(),
            duration_seconds,
            timestamp: now,
        },
        handoff: handoff_fields,
    };

    let toml_content = match toml::to_string_pretty(&artifact) {
        Ok(content) => content,
        Err(e) => {
            warn!("Failed to serialize handoff artifact: {e}");
            return;
        }
    };

    let handoff_path = session_dir.join("handoff.toml");
    if let Err(e) = fs::write(&handoff_path, toml_content) {
        warn!(
            path = %handoff_path.display(),
            error = %e,
            "Failed to write handoff.toml"
        );
    } else {
        info!(
            session = %session.meta_session_id,
            "Wrote handoff.toml"
        );
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_artifact_serializes_to_valid_toml() {
        let artifact = HandoffArtifact {
            session: HandoffSessionMeta {
                session_id: "01ABC".to_string(),
                branch: Some("feat/test".to_string()),
                description: Some("test session".to_string()),
                exit_code: 0,
                tool: "codex".to_string(),
                duration_seconds: 120,
                timestamp: chrono::Utc::now(),
            },
            handoff: HandoffFields {
                tried_and_worked: vec!["approach A".to_string()],
                tried_and_failed: vec!["approach B: too slow".to_string()],
                next_steps: vec!["implement C".to_string()],
                key_decisions: vec!["chose D over E".to_string()],
            },
        };

        let toml_str = toml::to_string_pretty(&artifact).expect("serialize");
        assert!(toml_str.contains("session_id = \"01ABC\""));
        assert!(toml_str.contains("tried_and_worked"));
        assert!(toml_str.contains("approach A"));
        assert!(toml_str.contains("tried_and_failed"));
        assert!(toml_str.contains("next_steps"));
        assert!(toml_str.contains("key_decisions"));
    }

    #[test]
    fn handoff_artifact_omits_empty_handoff_fields() {
        let artifact = HandoffArtifact {
            session: HandoffSessionMeta {
                session_id: "01XYZ".to_string(),
                branch: None,
                description: None,
                exit_code: 1,
                tool: "gemini".to_string(),
                duration_seconds: 60,
                timestamp: chrono::Utc::now(),
            },
            handoff: HandoffFields::default(),
        };

        let toml_str = toml::to_string_pretty(&artifact).expect("serialize");
        assert!(toml_str.contains("session_id"));
        // Empty vecs should be omitted
        assert!(!toml_str.contains("tried_and_worked"));
        assert!(!toml_str.contains("tried_and_failed"));
        assert!(!toml_str.contains("next_steps"));
        assert!(!toml_str.contains("key_decisions"));
        // Optional session fields should be absent
        assert!(!toml_str.contains("branch"));
        assert!(!toml_str.contains("description"));
    }

    #[test]
    fn write_handoff_artifact_creates_file_in_session_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_root = tmp.path();
        let session =
            csa_session::create_session(project_root, Some("handoff test"), None, Some("codex"))
                .expect("create session");
        let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id)
            .expect("session dir");

        let started_at = chrono::Utc::now() - chrono::Duration::seconds(45);
        let result = csa_process::ExecutionResult {
            exit_code: 0,
            output: String::new(),
            stderr_output: String::new(),
            summary: "test completed".to_string(),
        };

        write_handoff_artifact(&session_dir, &session, &result, "codex", started_at);

        let handoff_path = session_dir.join("handoff.toml");
        assert!(handoff_path.exists(), "handoff.toml should be created");

        let content = fs::read_to_string(&handoff_path).expect("read handoff.toml");
        assert!(content.contains(&session.meta_session_id));
        assert!(content.contains("exit_code = 0"));
        assert!(content.contains("tool = \"codex\""));
    }

    #[test]
    fn write_handoff_artifact_skips_when_session_dir_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_root = tmp.path();
        let missing_dir = project_root.join("nonexistent");

        let session =
            csa_session::create_session(project_root, Some("skip test"), None, Some("codex"))
                .expect("create session");

        let result = csa_process::ExecutionResult {
            exit_code: 0,
            output: String::new(),
            stderr_output: String::new(),
            summary: String::new(),
        };

        // Should not panic, just log warning
        write_handoff_artifact(&missing_dir, &session, &result, "codex", chrono::Utc::now());

        assert!(!missing_dir.join("handoff.toml").exists());
    }
}
