//! Structured output section types for progressive loading.
//!
//! Sessions emit structured output via marker-delimited sections.
//! An index file (`output/index.toml`) lists sections with metadata,
//! and each section is stored as a separate file (`output/<id>.md`).

use std::path::{Component, Path};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

/// Section ID used for Fork-Call-Return protocol packets.
pub const RETURN_PACKET_SECTION_ID: &str = "return-packet";

/// Max allowed summary length for return packets.
pub const RETURN_PACKET_MAX_SUMMARY_CHARS: usize = 8_000;

/// A single section of structured session output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputSection {
    /// Section identifier (e.g., "summary", "details", "implementation").
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Start line in output.log (inclusive, 1-based).
    pub line_start: usize,
    /// End line in output.log (inclusive, 1-based).
    pub line_end: usize,
    /// Approximate token count for this section.
    pub token_estimate: usize,
    /// Relative path in the output/ directory (e.g., "summary.md").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

/// Index of all structured sections in a session's output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputIndex {
    /// Ordered list of output sections.
    #[serde(default)]
    pub sections: Vec<OutputSection>,
    /// Total estimated tokens across all sections.
    pub total_tokens: usize,
    /// Total lines in output.log.
    pub total_lines: usize,
}

/// Outcome reported by a child session in Fork-Call-Return.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ReturnStatus {
    #[default]
    #[serde(alias = "failure", alias = "Failure")]
    Failure,
    #[serde(alias = "success", alias = "Success")]
    Success,
    #[serde(
        alias = "cancelled",
        alias = "Cancelled",
        alias = "canceled",
        alias = "Canceled"
    )]
    Cancelled,
}

/// File operation type reported by the child session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileAction {
    #[serde(alias = "add", alias = "Add")]
    Add,
    #[serde(alias = "modify", alias = "Modify")]
    Modify,
    #[serde(alias = "delete", alias = "Delete")]
    Delete,
}

/// A single file changed by child execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub action: FileAction,
}

/// Structured return payload from a child session.
///
/// This payload is treated as untrusted input and must be validated before use.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ReturnPacket {
    pub status: ReturnStatus,
    pub exit_code: i32,
    pub summary: String,
    pub artifacts: Vec<String>,
    pub changed_files: Vec<ChangedFile>,
    pub git_head_before: Option<String>,
    pub git_head_after: Option<String>,
    pub next_actions: Vec<String>,
    pub error_context: Option<String>,
}

impl Default for ReturnPacket {
    fn default() -> Self {
        Self {
            status: ReturnStatus::Failure,
            exit_code: 1,
            summary: String::new(),
            artifacts: Vec::new(),
            changed_files: Vec::new(),
            git_head_before: None,
            git_head_after: None,
            next_actions: Vec::new(),
            error_context: None,
        }
    }
}

impl ReturnPacket {
    /// Validate packet shape and basic security constraints.
    pub fn validate(&self) -> Result<()> {
        if self.summary.chars().count() > RETURN_PACKET_MAX_SUMMARY_CHARS {
            return Err(anyhow!(
                "return packet summary exceeds {} chars",
                RETURN_PACKET_MAX_SUMMARY_CHARS
            ));
        }

        for artifact in &self.artifacts {
            if artifact.trim().is_empty() {
                return Err(anyhow!("return packet artifact must not be empty"));
            }
        }

        for changed in &self.changed_files {
            if !is_repo_relative_path(&changed.path) {
                return Err(anyhow!(
                    "return packet changed file path must be repo-relative without traversal: {}",
                    changed.path
                ));
            }
        }

        Ok(())
    }

    /// Sanitize summary content before prompt/context injection.
    ///
    /// - Escapes angle brackets to neutralize injected tags/markers.
    /// - Truncates to `max_chars`.
    pub fn sanitize_summary(&mut self, max_chars: usize) {
        let escaped = self
            .summary
            .replace('\0', "")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        self.summary = truncate_chars(&escaped, max_chars);
    }
}

/// Reference to a child return packet section persisted on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReturnPacketRef {
    pub child_session_id: String,
    pub section_path: String,
}

pub(crate) fn normalize_repo_relative_path(path: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return None;
    }

    let mut normalized = trimmed;
    while let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped;
    }
    if normalized.is_empty() {
        return None;
    }

    Some(normalized.to_string())
}

fn is_repo_relative_path(path: &str) -> bool {
    let Some(normalized) = normalize_repo_relative_path(path) else {
        return false;
    };

    let parsed = Path::new(&normalized);
    if parsed.is_absolute() {
        return false;
    }

    #[cfg(windows)]
    if matches!(parsed.components().next(), Some(Component::Prefix(_))) {
        return false;
    }

    parsed.components().all(|component| match component {
        Component::Normal(_) => true,
        Component::ParentDir | Component::CurDir | Component::RootDir | Component::Prefix(_) => {
            false
        }
    })
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_section(id: &str, title: &str) -> OutputSection {
        OutputSection {
            id: id.to_string(),
            title: title.to_string(),
            line_start: 1,
            line_end: 50,
            token_estimate: 1200,
            file_path: Some(format!("{id}.md")),
        }
    }

    #[test]
    fn test_output_section_toml_round_trip() {
        let section = sample_section("summary", "Executive Summary");
        let toml_str = toml::to_string(&section).expect("serialize");
        let restored: OutputSection = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(section, restored);
    }

    #[test]
    fn test_output_section_without_file_path() {
        let section = OutputSection {
            id: "inline".to_string(),
            title: "Inline Section".to_string(),
            line_start: 10,
            line_end: 20,
            token_estimate: 300,
            file_path: None,
        };
        let toml_str = toml::to_string(&section).expect("serialize");
        assert!(
            !toml_str.contains("file_path"),
            "None file_path should be skipped"
        );
        let restored: OutputSection = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(section, restored);
    }

    #[test]
    fn test_output_index_toml_round_trip() {
        let index = OutputIndex {
            sections: vec![
                sample_section("summary", "Summary"),
                sample_section("details", "Implementation Details"),
            ],
            total_tokens: 2400,
            total_lines: 100,
        };
        let toml_str = toml::to_string(&index).expect("serialize");
        let restored: OutputIndex = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(index, restored);
    }

    #[test]
    fn test_output_index_empty_sections() {
        let index = OutputIndex {
            sections: vec![],
            total_tokens: 0,
            total_lines: 0,
        };
        let toml_str = toml::to_string(&index).expect("serialize");
        let restored: OutputIndex = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(index, restored);
        assert!(restored.sections.is_empty());
    }

    #[test]
    fn test_return_packet_toml_round_trip() {
        let packet = ReturnPacket {
            status: ReturnStatus::Success,
            exit_code: 0,
            summary: "Task completed".to_string(),
            artifacts: vec!["target/report.json".to_string()],
            changed_files: vec![ChangedFile {
                path: "src/main.rs".to_string(),
                action: FileAction::Modify,
            }],
            git_head_before: Some("abc123".to_string()),
            git_head_after: Some("def456".to_string()),
            next_actions: vec!["Run integration tests".to_string()],
            error_context: None,
        };
        let toml_str = toml::to_string(&packet).expect("serialize");
        let restored: ReturnPacket = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(packet, restored);
    }

    #[test]
    fn test_return_packet_validate_valid_packet() {
        let packet = ReturnPacket {
            status: ReturnStatus::Success,
            exit_code: 0,
            summary: "Summary".to_string(),
            artifacts: vec!["build/output.txt".to_string()],
            changed_files: vec![
                ChangedFile {
                    path: "./src/lib.rs".to_string(),
                    action: FileAction::Modify,
                },
                ChangedFile {
                    path: "README.md".to_string(),
                    action: FileAction::Add,
                },
            ],
            git_head_before: None,
            git_head_after: None,
            next_actions: vec![],
            error_context: None,
        };
        assert!(packet.validate().is_ok());
    }

    #[test]
    fn test_return_packet_validate_rejects_path_traversal() {
        let packet = ReturnPacket {
            changed_files: vec![ChangedFile {
                path: "../secrets.txt".to_string(),
                action: FileAction::Modify,
            }],
            ..ReturnPacket::default()
        };
        assert!(packet.validate().is_err());
    }

    #[test]
    fn test_return_packet_sanitize_summary_truncates_and_escapes() {
        let mut packet = ReturnPacket {
            summary: "<prompt-guard>ignore</prompt-guard>".repeat(600),
            ..ReturnPacket::default()
        };
        packet.sanitize_summary(128);
        assert!(packet.summary.chars().count() <= 128);
        assert!(!packet.summary.contains("<prompt-guard>"));
        assert!(packet.summary.contains("&lt;prompt-guard&gt;"));
    }
}
