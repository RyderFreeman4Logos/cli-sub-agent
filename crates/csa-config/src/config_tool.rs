use serde::{Deserialize, Serialize};

use super::config::EnforcementMode;

pub(crate) fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub restrictions: Option<ToolRestrictions>,
    /// Suppress notification hooks (default: true). Injects `CSA_SUPPRESS_NOTIFY=1`.
    #[serde(default = "default_true")]
    pub suppress_notify: bool,
    /// Per-tool sandbox enforcement mode override. Takes precedence over project resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement_mode: Option<EnforcementMode>,
    /// Per-tool memory limit override (MB). Takes precedence over project resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_max_mb: Option<u64>,
    /// Per-tool swap limit override (MB). Takes precedence over project resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_swap_max_mb: Option<u64>,
    /// Per-tool Node.js heap size limit (MB). Takes precedence over project resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_heap_limit_mb: Option<u64>,
    /// Deprecated: use `setting_sources` instead.
    /// When `true`, equivalent to `setting_sources = []` (load nothing).
    /// When `false` or absent, no override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean_mode: Option<bool>,
    /// Selective MCP/setting sources to load for ACP-backed tools.
    /// `Some(vec![])` = load nothing (equivalent to old `lean_mode = true`).
    /// `Some(vec!["project"])` = load only project-level settings.
    /// `None` = default (load everything).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<Vec<String>>,
    /// Default model override used when `--tool` is explicit but `--model` is omitted.
    ///
    /// Lower priority than `--model` and `--model-spec`. Higher priority than the
    /// tool's internal default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    /// Default thinking budget used when `--tool` is explicit but `--thinking` is omitted.
    ///
    /// Accepts the same values as `--thinking`: low, medium, high, xhigh, or a number.
    /// Lower priority than `--thinking` and `thinking_lock`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_thinking: Option<String>,
    /// Lock thinking budget for this tool. When set, any CLI `--thinking` or
    /// `--model-spec` thinking override is silently replaced with this value.
    /// Accepts the same values as `--thinking`: low, medium, high, xhigh, or a number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_lock: Option<String>,
    /// Codex-only: auto-approve trust dialog during PTY native fork flow.
    /// Defaults to false for explicit safety.
    #[serde(default)]
    pub codex_auto_trust: bool,
    /// OpenAI-compat only: base URL for the API endpoint (e.g., "http://localhost:8317").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// API key for authentication. Used by openai-compat and gemini-cli (fallback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            restrictions: None,
            suppress_notify: true,
            enforcement_mode: None,
            memory_max_mb: None,
            memory_swap_max_mb: None,
            node_heap_limit_mb: None,
            lean_mode: None,
            setting_sources: None,
            default_model: None,
            default_thinking: None,
            thinking_lock: None,
            codex_auto_trust: false,
            base_url: None,
            api_key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRestrictions {
    #[serde(default)]
    pub allow_edit_existing_files: bool,
}
