//! Session metadata stored separately from state.toml.

use serde::{Deserialize, Serialize};

/// Session metadata stored in metadata.toml (separate from state.toml).
/// Written once at session creation time. The tool field is immutable after creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// The tool that owns this session (e.g., "codex", "claude-code")
    pub tool: String,
    /// Whether the tool is locked (prevents other tools from using this session)
    #[serde(default = "default_tool_locked")]
    pub tool_locked: bool,
}

fn default_tool_locked() -> bool {
    true
}

pub const METADATA_FILE_NAME: &str = "metadata.toml";

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ── Serialization round-trip ───────────────────────────────────

    #[test]
    fn test_metadata_roundtrip_toml() {
        let metadata = SessionMetadata {
            tool: "codex".to_string(),
            tool_locked: true,
        };

        let toml_str = toml::to_string_pretty(&metadata).expect("Serialize should succeed");
        let deserialized: SessionMetadata =
            toml::from_str(&toml_str).expect("Deserialize should succeed");

        assert_eq!(deserialized.tool, "codex");
        assert!(deserialized.tool_locked);
    }

    #[test]
    fn test_metadata_save_load_via_file() {
        let tmp = tempdir().expect("Failed to create temp dir");
        let path = tmp.path().join(METADATA_FILE_NAME);

        let metadata = SessionMetadata {
            tool: "claude-code".to_string(),
            tool_locked: false,
        };

        let contents = toml::to_string_pretty(&metadata).unwrap();
        std::fs::write(&path, &contents).expect("Write should succeed");

        let read_back = std::fs::read_to_string(&path).expect("Read should succeed");
        let loaded: SessionMetadata = toml::from_str(&read_back).expect("Parse should succeed");

        assert_eq!(loaded.tool, "claude-code");
        assert!(!loaded.tool_locked);
    }

    // ── Default for tool_locked ────────────────────────────────────

    #[test]
    fn test_tool_locked_defaults_to_true() {
        // Deserialize TOML without tool_locked field — should default to true
        let toml_str = r#"tool = "gemini-cli""#;
        let metadata: SessionMetadata =
            toml::from_str(toml_str).expect("Deserialize should succeed");

        assert_eq!(metadata.tool, "gemini-cli");
        assert!(
            metadata.tool_locked,
            "tool_locked should default to true when absent"
        );
    }

    // ── Error path: missing required field ─────────────────────────

    #[test]
    fn test_metadata_missing_tool_field_errors() {
        let toml_str = r#"tool_locked = false"#;
        let result = toml::from_str::<SessionMetadata>(toml_str);
        assert!(result.is_err(), "Missing 'tool' field should error");
    }
}
