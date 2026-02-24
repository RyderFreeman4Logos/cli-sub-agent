//! Global configuration for CLI Sub-Agent (`~/.config/cli-sub-agent/config.toml`).
//!
//! Stores user-level settings that apply across all projects:
//! - Per-tool concurrency limits (slot counts)
//! - API keys and environment variables injected into child processes
//!
//! Completely separate from project config (`{project}/.csa/config.toml`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use csa_core::types::ToolName;

use crate::mcp::McpServerConfig;
use crate::paths;

/// Default maximum concurrent instances per tool.
const DEFAULT_MAX_CONCURRENT: u32 = 3;

/// Global configuration loaded from `~/.config/cli-sub-agent/config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalConfig {
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub preferences: PreferencesConfig,
    #[serde(default)]
    pub tools: HashMap<String, GlobalToolConfig>,
    #[serde(default)]
    pub review: ReviewConfig,
    #[serde(default)]
    pub debate: DebateConfig,
    #[serde(default)]
    pub fallback: FallbackConfig,
    #[serde(default)]
    pub todo: TodoDisplayConfig,
    /// Global MCP server registry injected into all tool sessions.
    ///
    /// Merged with project-level `.csa/mcp.toml` servers (project takes precedence
    /// for same-name servers).
    #[serde(default)]
    pub mcp: GlobalMcpConfig,
    /// Optional MCP hub unix socket path for shared proxy mode.
    ///
    /// When set, ACP sessions may inject a single mcp-hub endpoint instead of
    /// individual MCP server entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_proxy_socket: Option<String>,
}

/// User preferences for tool selection and routing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PreferencesConfig {
    /// Tool priority order for auto-selection. First = most preferred.
    ///
    /// Affects: heterogeneous candidate ordering, reviewer allocation,
    /// any-available fallback. Does NOT affect explicit `--tool` overrides
    /// or tier model declaration order.
    ///
    /// Tools not listed are appended in their default order.
    /// Empty list (default) preserves existing behavior.
    #[serde(default)]
    pub tool_priority: Vec<String>,
}

/// Configuration for the code review workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewConfig {
    /// Review tool selection: "auto", "codex", "claude-code", "opencode", "gemini-cli".
    ///
    /// In `auto` mode, the review tool is the heterogeneous counterpart of the parent:
    /// - Parent is `claude-code` → review with `codex`
    /// - Parent is `codex` → review with `claude-code`
    /// - Otherwise → error (user must configure explicitly)
    #[serde(default = "default_review_tool")]
    pub tool: String,
}

fn default_review_tool() -> String {
    "auto".to_string()
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            tool: default_review_tool(),
        }
    }
}

/// Configuration for the debate workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateConfig {
    /// Debate tool selection: "auto", "codex", "claude-code", "opencode", "gemini-cli".
    ///
    /// In `auto` mode, the debate tool is the heterogeneous counterpart of the parent:
    /// - Parent is `claude-code` → debate with `codex`
    /// - Parent is `codex` → debate with `claude-code`
    /// - Otherwise → error (user must configure explicitly)
    #[serde(default = "default_debate_tool")]
    pub tool: String,
    /// Default absolute wall-clock timeout (seconds) for `csa debate`.
    ///
    /// `csa debate --timeout <N>` overrides this per invocation.
    #[serde(default = "default_debate_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Default thinking budget for `csa debate` (`low`, `medium`, `high`, `xhigh`).
    ///
    /// `csa debate --thinking <LEVEL>` overrides this per invocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Allow same-model adversarial fallback when heterogeneous models are unavailable.
    ///
    /// When enabled (default), `csa debate` falls back to running two independent
    /// sub-agents of the same tool as Proposer and Critic. The debate output is
    /// annotated with "same-model adversarial" to indicate degraded diversity.
    ///
    /// Set to `false` to require heterogeneous models (strict mode).
    #[serde(default = "default_true_debate")]
    pub same_model_fallback: bool,
}

fn default_debate_tool() -> String {
    "auto".to_string()
}

fn default_debate_timeout_seconds() -> u64 {
    1800
}

fn default_true_debate() -> bool {
    true
}

impl Default for DebateConfig {
    fn default() -> Self {
        Self {
            tool: default_debate_tool(),
            timeout_seconds: default_debate_timeout_seconds(),
            thinking: None,
            same_model_fallback: true,
        }
    }
}

/// Configuration for fallback behavior when external services are unavailable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackConfig {
    /// Behavior when cloud review bot is unavailable (quota, timeout, or API errors).
    ///
    /// - `"auto-local"`: Automatically fall back to local CSA review (still reviews)
    /// - `"ask-user"`: Prompt user before falling back (default)
    ///
    /// Both policies ensure code is still reviewed — `auto-local` just skips the
    /// user confirmation prompt. There is no `skip` option because bypassing
    /// review entirely violates the heterogeneous review safety model.
    #[serde(default = "default_cloud_review_exhausted")]
    pub cloud_review_exhausted: String,
}

fn default_cloud_review_exhausted() -> String {
    "ask-user".to_string()
}

impl Default for FallbackConfig {
    fn default() -> Self {
        Self {
            cloud_review_exhausted: default_cloud_review_exhausted(),
        }
    }
}

/// Display configuration for `csa todo` subcommands.
///
/// When set, output is piped through the specified external command.
/// Falls back to plain `print!()` when the command is absent or stdout is not a terminal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TodoDisplayConfig {
    /// Command to pipe `csa todo show` output through (e.g., `"bat -l md"`).
    #[serde(default)]
    pub show_command: Option<String>,
    /// Command to pipe `csa todo diff` output through (e.g., `"delta"`).
    #[serde(default)]
    pub diff_command: Option<String>,
}

/// Global MCP server configuration.
///
/// Servers listed here are injected into every spawned tool session.
/// Project-level `.csa/mcp.toml` servers override global ones with the same name.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalMcpConfig {
    /// MCP servers available to all tool sessions.
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

/// Returns the heterogeneous counterpart tool for model-diversity enforcement.
///
/// - `claude-code` → `codex`
/// - `codex` → `claude-code`
/// - Anything else → `None`
pub fn heterogeneous_counterpart(tool: &str) -> Option<&'static str> {
    match tool {
        "claude-code" => Some("codex"),
        "codex" => Some("claude-code"),
        _ => None,
    }
}

/// Select a tool from a different model family than the given tool.
/// Returns None if no heterogeneous tool is available.
pub fn select_heterogeneous_tool(
    parent_tool: &ToolName,
    available_tools: &[ToolName],
) -> Option<ToolName> {
    let parent_family = parent_tool.model_family();
    available_tools
        .iter()
        .find(|t| t.model_family() != parent_family)
        .copied()
}

/// Returns all known tool names as a static slice.
pub fn all_known_tools() -> &'static [ToolName] {
    &[
        ToolName::GeminiCli,
        ToolName::Opencode,
        ToolName::Codex,
        ToolName::ClaudeCode,
    ]
}

/// Sort tools by a priority list. Listed tools appear first (in priority order).
/// Unlisted tools retain their original relative order, appended after listed ones.
///
/// Returns the input unchanged when `priority` is empty (backward compatible).
pub fn sort_tools_by_priority(tools: &[ToolName], priority: &[String]) -> Vec<ToolName> {
    if priority.is_empty() {
        return tools.to_vec();
    }
    let mut result = tools.to_vec();
    result.sort_by_key(|tool| {
        priority
            .iter()
            .position(|p| p == tool.as_str())
            .unwrap_or(priority.len())
    });
    result
}

/// Resolve effective tool priority: project-level overrides global when present.
pub fn effective_tool_priority<'a>(
    project_config: Option<&'a crate::ProjectConfig>,
    global_config: &'a GlobalConfig,
) -> &'a [String] {
    project_config
        .and_then(|p| p.preferences.as_ref())
        .map(|p| p.tool_priority.as_slice())
        .filter(|p| !p.is_empty())
        .unwrap_or(&global_config.preferences.tool_priority)
}

/// Sort tools using effective priority from project (if set) or global config.
pub fn sort_tools_by_effective_priority(
    tools: &[ToolName],
    project_config: Option<&crate::ProjectConfig>,
    global_config: &GlobalConfig,
) -> Vec<ToolName> {
    sort_tools_by_priority(
        tools,
        effective_tool_priority(project_config, global_config),
    )
}

/// Global defaults section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultsConfig {
    /// Default maximum concurrent instances per tool (default: 3).
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u32,
    /// Default parent tool context when auto-detection fails.
    #[serde(default)]
    pub tool: Option<String>,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            tool: None,
        }
    }
}

/// Per-tool global configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalToolConfig {
    /// Maximum concurrent instances for this tool. None = use defaults.
    #[serde(default)]
    pub max_concurrent: Option<u32>,
    /// Environment variables injected into child processes for this tool.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Per-tool memory limit override (MB). Takes precedence over project/global resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_max_mb: Option<u64>,
    /// Per-tool swap limit override (MB). Takes precedence over project/global resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_swap_max_mb: Option<u64>,
    /// Lock thinking budget for this tool. When set, any CLI `--thinking` or
    /// `--model-spec` thinking override is silently replaced with this value.
    /// Accepts: low, medium, high, xhigh, or a numeric token count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_lock: Option<String>,
}

fn default_max_concurrent() -> u32 {
    DEFAULT_MAX_CONCURRENT
}

impl GlobalConfig {
    /// Load global config from `~/.config/cli-sub-agent/config.toml`.
    ///
    /// Returns `Default` if the file does not exist or if the config
    /// directory cannot be determined (e.g., no HOME in containers).
    pub fn load() -> Result<Self> {
        let path = match paths::config_dir() {
            Some(dir) => dir.join("config.toml"),
            None => return Ok(Self::default()),
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read global config: {}", path.display()))?;
        let config: Self = toml::from_str(&content)
            .with_context(|| format!("Failed to parse global config: {}", path.display()))?;
        Ok(config)
    }

    /// Get the resolved maximum concurrent count for a tool.
    ///
    /// Lookup order: tool-specific override -> defaults.max_concurrent.
    pub fn max_concurrent(&self, tool: &str) -> u32 {
        self.tools
            .get(tool)
            .and_then(|t| t.max_concurrent)
            .unwrap_or(self.defaults.max_concurrent)
    }

    /// Sort tools by user-configured priority order.
    ///
    /// Tools in `preferences.tool_priority` appear first (in priority order).
    /// Tools NOT in the priority list retain their original relative order.
    /// Returns unchanged when no priority is configured.
    pub fn sort_by_priority(&self, tools: &[ToolName]) -> Vec<ToolName> {
        sort_tools_by_priority(tools, &self.preferences.tool_priority)
    }

    /// Get environment variables to inject for a tool.
    pub fn env_vars(&self, tool: &str) -> Option<&HashMap<String, String>> {
        self.tools
            .get(tool)
            .map(|t| &t.env)
            .filter(|m| !m.is_empty())
    }

    /// Get the thinking budget lock for a tool from global config.
    pub fn thinking_lock(&self, tool: &str) -> Option<&str> {
        self.tools
            .get(tool)
            .and_then(|t| t.thinking_lock.as_deref())
    }

    /// Get globally configured MCP servers.
    pub fn mcp_servers(&self) -> &[McpServerConfig] {
        &self.mcp.servers
    }

    /// Path to the global config file: `~/.config/cli-sub-agent/config.toml`.
    pub fn config_path() -> Result<PathBuf> {
        let dir = paths::config_dir_write().context("Failed to determine config directory")?;
        Ok(dir.join("config.toml"))
    }

    /// Path to the global slots directory.
    ///
    /// Base state directory for all CSA data (`~/.local/state/cli-sub-agent/`).
    ///
    /// Used by `--global` GC to scan all project session trees.
    pub fn state_base_dir() -> Result<PathBuf> {
        let base = paths::state_dir().unwrap_or_else(paths::state_dir_fallback);
        Ok(base)
    }

    /// Resolution order:
    /// 1. `~/.local/state/cli-sub-agent/slots/` (XDG state dir on Linux)
    /// 2. Platform-equivalent state dir (macOS/Windows)
    /// 3. `$TMPDIR/cli-sub-agent-state/slots/` (fallback when state_dir unavailable)
    /// 4. `$TMPDIR/cli-sub-agent-state/slots/` (fallback when HOME/XDG unset, e.g. containers)
    ///
    /// This function never fails — it always returns a usable path.
    pub fn slots_dir() -> Result<PathBuf> {
        let base = paths::state_dir_write().unwrap_or_else(paths::state_dir_fallback);
        Ok(base.join("slots"))
    }

    /// Generate default config TOML with comments as a template.
    pub fn default_template() -> String {
        r#"# CSA Global Configuration
# Location: ~/.config/cli-sub-agent/config.toml
#
# This file controls system-wide settings for all CSA projects.
# API keys and concurrency limits are configured here (not in project config).

[defaults]
max_concurrent = 3  # Default max parallel instances per tool
# tool = "codex"  # Default tool when auto-detection fails

# Per-tool overrides. Uncomment and configure as needed.
#
# [tools.gemini-cli]
# max_concurrent = 5  # Higher limit with API key
# [tools.gemini-cli.env]
# GEMINI_API_KEY = "AI..."
#
# [tools.claude-code]
# max_concurrent = 1
# [tools.claude-code.env]
# ANTHROPIC_API_KEY = "sk-ant-..."
#
# [tools.codex]
# max_concurrent = 3
# [tools.codex.env]
# OPENAI_API_KEY = "sk-..."
#
# [tools.opencode]
# max_concurrent = 2
# [tools.opencode.env]
# ANTHROPIC_API_KEY = "sk-ant-..."

# Tool priority for auto-selection (heterogeneous routing, review, debate).
# First = most preferred. Tools not listed keep their default order.
# Example: prefer Claude Code for worker tasks, then Codex.
# [preferences]
# tool_priority = ["claude-code", "codex", "gemini-cli", "opencode"]

# Review workflow: which tool to use for code review.
# "auto" selects the heterogeneous counterpart of the parent tool:
#   claude-code parent -> codex, codex parent -> claude-code.
# Set explicitly if auto-detection fails (e.g., parent is opencode).
[review]
tool = "auto"

# Debate workflow: which tool to use for adversarial debate / arbitration.
# "auto" selects the heterogeneous counterpart of the parent tool:
#   claude-code parent -> codex, codex parent -> claude-code.
# Set explicitly if auto-detection fails (e.g., parent is opencode).
[debate]
tool = "auto"
# Default wall-clock timeout for `csa debate` (30 minutes).
timeout_seconds = 1800
# Optional default thinking budget for `csa debate`.
# thinking = "high"
# Allow same-model adversarial fallback when heterogeneous models are unavailable.
# When true, `csa debate` runs two independent sub-agents of the same tool.
# Output is annotated with "same-model adversarial" to indicate degraded diversity.
same_model_fallback = true

# Fallback behavior when external services are unavailable.
# cloud_review_exhausted: what to do when cloud review bot is unavailable.
#   "auto-local" = automatically fall back to local CSA review (still reviews)
#   "ask-user"   = prompt user before falling back (default)
[fallback]
cloud_review_exhausted = "ask-user"

# Display commands for `csa todo` subcommands.
# When set, output is piped through the specified command (only when stdout is a terminal).
# [todo]
# show_command = "bat -l md"   # Pipe `csa todo show` output through bat
# diff_command = "delta"       # Pipe `csa todo diff` output through delta

# MCP (Model Context Protocol) servers injected into all tool sessions.
# Project-level .csa/mcp.toml servers override global ones with the same name.
#
# Stdio transport (local process, default):
# [[mcp.servers]]
# name = "repomix"
# type = "stdio"
# command = "npx"
# args = ["-y", "repomix", "--mcp"]
#
# HTTP transport (remote server, requires transport-http-client feature):
# [[mcp.servers]]
# name = "remote-mcp"
# type = "http"
# url = "https://mcp.example.com/mcp"
# # headers = { Authorization = "Bearer ..." }
# # allow_insecure = false  # Set true for http:// (not recommended)
#
# Legacy format (auto-detected as stdio, backward-compatible):
# [[mcp.servers]]
# name = "deepwiki"
# command = "npx"
# args = ["-y", "@anthropic/deepwiki-mcp"]
#
# Optional shared MCP hub socket path.
# mcp_proxy_socket = "/run/user/1000/cli-sub-agent/mcp-hub.sock"
"#
        .to_string()
    }

    /// Save the default template to the config path, creating directories as needed.
    /// Returns the path where the file was written.
    pub fn save_default_template() -> Result<PathBuf> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }
        std::fs::write(&path, Self::default_template())
            .with_context(|| format!("Failed to write global config: {}", path.display()))?;
        Ok(path)
    }

    /// Resolve the review tool based on config and parent tool context.
    ///
    /// In `auto` mode:
    /// - Parent is `claude-code` → `codex` (model heterogeneity)
    /// - Parent is `codex` → `claude-code`
    /// - Otherwise → error with guidance to configure manually
    pub fn resolve_review_tool(&self, parent_tool: Option<&str>) -> Result<String> {
        if self.review.tool != "auto" {
            return Ok(self.review.tool.clone());
        }
        resolve_auto_tool("review", parent_tool)
    }

    /// Resolve the debate tool based on config and parent tool context.
    ///
    /// In `auto` mode:
    /// - Parent is `claude-code` → `codex` (model heterogeneity)
    /// - Parent is `codex` → `claude-code`
    /// - Otherwise → error with guidance to configure manually
    pub fn resolve_debate_tool(&self, parent_tool: Option<&str>) -> Result<String> {
        if self.debate.tool != "auto" {
            return Ok(self.debate.tool.clone());
        }
        resolve_auto_tool("debate", parent_tool)
    }

    /// List all known tool names (from config + static list).
    pub fn all_tool_slots(&self) -> Vec<(&str, u32)> {
        let static_tools = ["gemini-cli", "opencode", "codex", "claude-code"];
        let mut result: Vec<(&str, u32)> = static_tools
            .iter()
            .map(|t| (*t, self.max_concurrent(t)))
            .collect();

        // Add any extra tools from config not in static list
        for tool in self.tools.keys() {
            if !static_tools.contains(&tool.as_str()) {
                result.push((tool.as_str(), self.max_concurrent(tool)));
            }
        }

        result
    }
}

/// Resolve "auto" tool selection using the heterogeneous counterpart mapping.
fn resolve_auto_tool(section: &str, parent_tool: Option<&str>) -> Result<String> {
    match parent_tool.and_then(heterogeneous_counterpart) {
        Some(counterpart) => Ok(counterpart.to_string()),
        None => {
            let context = match parent_tool {
                Some(p) => format!("parent is '{}'", p),
                None => "no parent tool context".to_string(),
            };
            Err(anyhow::anyhow!(
                "Cannot auto-detect {section} tool: {context}. \
                 Set [{section}] tool to an explicit tool (e.g., \"codex\" or \"claude-code\") \
                 in ~/.config/cli-sub-agent/config.toml"
            ))
        }
    }
}

#[cfg(test)]
#[path = "global_tests.rs"]
mod tests;
