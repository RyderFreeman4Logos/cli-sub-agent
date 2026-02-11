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
