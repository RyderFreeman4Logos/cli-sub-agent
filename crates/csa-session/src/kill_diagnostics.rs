use serde::{Deserialize, Serialize};

/// Structured signal-kill diagnostics surfaced in `result.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KillDiagnosticReport {
    /// Concrete source classification, for example `memory_soft_limit`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_max_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_limit_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_name: Option<String>,
}
