use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::config::EnforcementMode;

pub(crate) fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransportKind {
    Auto,
    Cli,
    Acp,
}

pub fn default_transport_for_tool(tool_name: &str) -> Option<TransportKind> {
    match tool_name {
        "claude-code" | "codex" => Some(TransportKind::Acp),
        "gemini-cli" | "opencode" => Some(TransportKind::Cli),
        _ => None,
    }
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
    /// Per-tool initial-response timeout override (seconds).
    ///
    /// When set, it overrides `resources.initial_response_timeout_seconds` for
    /// this tool. `None` means fall back to the generic resources timeout (or a
    /// tool-specific default when the runtime defines one). `0` explicitly
    /// disables the initial-response watchdog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_response_timeout_seconds: Option<u64>,
    /// Optional tool transport override.
    ///
    /// Currently meaningful for codex only. `None` means use the build default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportKind>,
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
    /// Per-tool filesystem sandbox overrides. When set, replaces global
    /// filesystem sandbox settings for this specific tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem_sandbox: Option<ToolFilesystemSandboxConfig>,
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
            initial_response_timeout_seconds: None,
            transport: None,
            codex_auto_trust: false,
            base_url: None,
            api_key: None,
            filesystem_sandbox: None,
        }
    }
}

impl ToolConfig {
    #[must_use]
    pub fn resolve_transport(&self, tool_name: &str) -> Option<TransportKind> {
        match self.transport.unwrap_or(TransportKind::Auto) {
            TransportKind::Auto => default_transport_for_tool(tool_name),
            transport => Some(transport),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRestrictions {
    /// When false, the tool may not modify existing tracked files.
    /// Enforced via prompt injection + git-based edit guard.
    #[serde(default = "super::config_tool::default_true")]
    pub allow_edit_existing_files: bool,
    /// When false, the tool may not create new files.
    /// Combined with `allow_edit_existing_files = false` for full read-only mode.
    /// Enforced via prompt injection + git-based new-file guard.
    #[serde(default = "super::config_tool::default_true")]
    pub allow_write_new_files: bool,
}

impl Default for ToolRestrictions {
    fn default() -> Self {
        Self {
            allow_edit_existing_files: true,
            allow_write_new_files: true,
        }
    }
}

/// Per-tool filesystem sandbox configuration.
/// When `writable_paths` is set, it REPLACES the default project root
/// writable access (REPLACE semantics). When `readable_paths` is set, it
/// REPLACES the global `extra_readable` list for the tool. Session dir and tool
/// config dirs are always preserved regardless.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolFilesystemSandboxConfig {
    /// Writable paths for this tool. When set, REPLACES the default
    /// project root writable access. Use `["/tmp"]` to restrict a tool
    /// to only writing to /tmp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writable_paths: Option<Vec<PathBuf>>,

    /// Read-only host paths for this tool. When set, REPLACES the global
    /// `extra_readable` list for the tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readable_paths: Option<Vec<PathBuf>>,

    /// Per-tool enforcement mode override. When set, overrides the
    /// global `[filesystem_sandbox].enforcement_mode` for this tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement_mode: Option<String>,
}

impl ToolFilesystemSandboxConfig {
    /// Check if config is at default values (all None).
    /// Required by serde(default) rule — default values are
    /// indistinguishable from "not set".
    pub fn is_default(&self) -> bool {
        self.writable_paths.is_none()
            && self.readable_paths.is_none()
            && self.enforcement_mode.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_tool_filesystem_sandbox_is_default() {
        let cfg = ToolFilesystemSandboxConfig::default();
        assert!(cfg.is_default());

        let cfg = ToolFilesystemSandboxConfig {
            writable_paths: Some(vec![PathBuf::from("/tmp")]),
            readable_paths: None,
            enforcement_mode: None,
        };
        assert!(!cfg.is_default());

        let cfg = ToolFilesystemSandboxConfig {
            writable_paths: None,
            readable_paths: None,
            enforcement_mode: Some("required".to_string()),
        };
        assert!(!cfg.is_default());

        let cfg = ToolFilesystemSandboxConfig {
            writable_paths: None,
            readable_paths: Some(vec![PathBuf::from("/tmp/foo.json")]),
            enforcement_mode: Some("required".to_string()),
        };
        assert!(!cfg.is_default());
    }

    #[test]
    fn test_deserialize_tool_filesystem_sandbox() {
        #[derive(Deserialize)]
        struct Wrapper {
            tools: HashMap<String, ToolConfig>,
        }

        let toml_str = r#"
[tools.gemini-cli]
enabled = true

[tools.gemini-cli.filesystem_sandbox]
writable_paths = ["/tmp"]
readable_paths = ["/tmp/resp_ccp.json"]
enforcement_mode = "required"
"#;
        let wrapper: Wrapper = toml::from_str(toml_str).expect("should parse TOML");
        let tool = wrapper.tools.get("gemini-cli").expect("gemini-cli missing");
        let fs_sandbox = tool
            .filesystem_sandbox
            .as_ref()
            .expect("filesystem_sandbox missing");

        assert_eq!(fs_sandbox.writable_paths, Some(vec![PathBuf::from("/tmp")]));
        assert_eq!(
            fs_sandbox.readable_paths,
            Some(vec![PathBuf::from("/tmp/resp_ccp.json")])
        );
        assert_eq!(fs_sandbox.enforcement_mode, Some("required".to_string()));
        assert!(!fs_sandbox.is_default());
    }

    #[test]
    fn test_deserialize_tool_without_filesystem_sandbox() {
        #[derive(Deserialize)]
        struct Wrapper {
            tools: HashMap<String, ToolConfig>,
        }

        let toml_str = r#"
[tools.claude-code]
enabled = true
"#;
        let wrapper: Wrapper = toml::from_str(toml_str).expect("should parse TOML");
        let tool = wrapper
            .tools
            .get("claude-code")
            .expect("claude-code missing");
        assert!(tool.filesystem_sandbox.is_none());
    }

    #[test]
    fn test_deserialize_tool_transport_override() {
        #[derive(Deserialize)]
        struct Wrapper {
            tools: HashMap<String, ToolConfig>,
        }

        let toml_str = r#"
[tools.codex]
transport = "cli"
"#;
        let wrapper: Wrapper = toml::from_str(toml_str).expect("should parse TOML");
        let tool = wrapper.tools.get("codex").expect("codex missing");

        assert_eq!(tool.transport, Some(TransportKind::Cli));
    }

    #[test]
    fn test_resolve_transport_maps_auto_to_tool_default() {
        let config = ToolConfig {
            transport: Some(TransportKind::Auto),
            ..Default::default()
        };

        assert_eq!(
            config.resolve_transport("claude-code"),
            Some(TransportKind::Acp)
        );
        assert_eq!(
            config.resolve_transport("gemini-cli"),
            Some(TransportKind::Cli)
        );
    }

    #[test]
    fn test_resolve_transport_uses_default_when_unset() {
        let config = ToolConfig::default();

        assert_eq!(config.resolve_transport("codex"), Some(TransportKind::Acp));
        assert_eq!(
            config.resolve_transport("opencode"),
            Some(TransportKind::Cli)
        );
    }
}
