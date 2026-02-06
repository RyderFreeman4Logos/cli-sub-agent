//! Session state types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Meta-session state representing a logical work session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaSessionState {
    /// ULID identifier (26 characters, Crockford Base32)
    pub meta_session_id: String,

    /// Human-readable description (optional)
    pub description: Option<String>,

    /// Absolute path to the project directory
    pub project_path: String,

    /// When this session was created
    pub created_at: DateTime<Utc>,

    /// When this session was last accessed
    pub last_accessed: DateTime<Utc>,

    /// Genealogy information (parent, depth)
    #[serde(default)]
    pub genealogy: Genealogy,

    /// Tool-specific state (provider session IDs, etc.)
    #[serde(default)]
    pub tools: HashMap<String, ToolState>,

    /// Context compaction status
    #[serde(default)]
    pub context_status: ContextStatus,

    /// Cumulative token usage across all tools in this session
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_token_usage: Option<TokenUsage>,
}

/// Genealogy tracking for session parent-child relationships
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Genealogy {
    /// Parent session ID (None for root sessions)
    pub parent_session_id: Option<String>,

    /// Depth in the genealogy tree (0 for root sessions)
    pub depth: u32,
    // Note: Children are discovered dynamically via scanning, not stored here
}

/// Per-tool state within a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolState {
    /// Provider-specific session ID (e.g., Codex thread_id, Gemini session)
    /// None on first run before provider session is created
    pub provider_session_id: Option<String>,

    /// Summary of the last action performed by this tool
    pub last_action_summary: String,

    /// Exit code of the last tool invocation
    pub last_exit_code: i32,

    /// When this tool state was last updated
    pub updated_at: DateTime<Utc>,

    /// Token usage for this tool in this session
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsage>,
}

/// Token usage tracking for AI tool execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input tokens consumed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,

    /// Output tokens generated
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,

    /// Total tokens (input + output)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,

    /// Estimated cost in USD
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<f64>,
}

/// Context compaction status tracking
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextStatus {
    /// Whether the context has been compacted
    pub is_compacted: bool,

    /// When the context was last compacted (if ever)
    pub last_compacted_at: Option<DateTime<Utc>>,
}
