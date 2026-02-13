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

/// Default maximum concurrent instances per tool.
const DEFAULT_MAX_CONCURRENT: u32 = 3;

/// Global configuration loaded from `~/.config/cli-sub-agent/config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalConfig {
    #[serde(default)]
    pub defaults: GlobalDefaults,
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
}

fn default_debate_tool() -> String {
    "auto".to_string()
}

impl Default for DebateConfig {
    fn default() -> Self {
        Self {
            tool: default_debate_tool(),
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

/// Global defaults section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalDefaults {
    /// Default maximum concurrent instances per tool (default: 3).
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u32,
}

impl Default for GlobalDefaults {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT,
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
        let path = match Self::config_path() {
            Ok(p) => p,
            Err(_) => return Ok(Self::default()),
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

    /// Get environment variables to inject for a tool.
    pub fn env_vars(&self, tool: &str) -> Option<&HashMap<String, String>> {
        self.tools
            .get(tool)
            .map(|t| &t.env)
            .filter(|m| !m.is_empty())
    }

    /// Path to the global config file: `~/.config/cli-sub-agent/config.toml`.
    pub fn config_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "cli-sub-agent")
            .context("Failed to determine config directory")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Path to the global slots directory.
    ///
    /// Base state directory for all CSA data (`~/.local/state/csa/`).
    ///
    /// Used by `--global` GC to scan all project session trees.
    pub fn state_base_dir() -> Result<PathBuf> {
        let base = directories::ProjectDirs::from("", "", "csa")
            .map(|dirs| {
                // Mirror get_session_root fallback: state_dir → data_local_dir
                dirs.state_dir()
                    .unwrap_or_else(|| dirs.data_local_dir())
                    .to_path_buf()
            })
            .unwrap_or_else(|| std::env::temp_dir().join("csa-state"));
        Ok(base)
    }

    /// Resolution order:
    /// 1. `~/.local/state/csa/slots/` (XDG state dir on Linux)
    /// 2. Platform-equivalent state dir (macOS/Windows)
    /// 3. `$TMPDIR/csa-state/slots/` (fallback when state_dir unavailable)
    /// 4. `$TMPDIR/csa-state/slots/` (fallback when HOME/XDG unset, e.g. containers)
    ///
    /// This function never fails — it always returns a usable path.
    pub fn slots_dir() -> Result<PathBuf> {
        Ok(Self::state_base_dir()?.join("slots"))
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
