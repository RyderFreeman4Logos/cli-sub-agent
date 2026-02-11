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
