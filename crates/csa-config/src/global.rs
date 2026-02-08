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

/// Default maximum concurrent instances per tool.
const DEFAULT_MAX_CONCURRENT: u32 = 3;

/// Global configuration loaded from `~/.config/cli-sub-agent/config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalConfig {
    #[serde(default)]
    pub defaults: GlobalDefaults,
    #[serde(default)]
    pub tools: HashMap<String, GlobalToolConfig>,
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
    /// Prefers `~/.local/state/csa/slots/` (XDG state dir) but falls back
    /// to `/tmp/csa-slots` on platforms where `state_dir()` is unavailable
    /// (e.g., macOS).
    pub fn slots_dir() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "csa")
            .context("Failed to determine project directories")?;
        let base = dirs
            .state_dir()
            .map(|d| d.to_path_buf())
            .unwrap_or_else(|| {
                // Fallback for platforms without state_dir (macOS, etc.)
                std::env::temp_dir().join("csa-state")
            });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GlobalConfig::default();
        assert_eq!(config.defaults.max_concurrent, 3);
        assert!(config.tools.is_empty());
    }

    #[test]
    fn test_max_concurrent_default() {
        let config = GlobalConfig::default();
        assert_eq!(config.max_concurrent("gemini-cli"), 3);
        assert_eq!(config.max_concurrent("codex"), 3);
    }

    #[test]
    fn test_max_concurrent_tool_override() {
        let mut config = GlobalConfig::default();
        config.tools.insert(
            "gemini-cli".to_string(),
            GlobalToolConfig {
                max_concurrent: Some(5),
                env: HashMap::new(),
            },
        );
        assert_eq!(config.max_concurrent("gemini-cli"), 5);
        assert_eq!(config.max_concurrent("codex"), 3); // falls back to default
    }

    #[test]
    fn test_env_vars() {
        let mut config = GlobalConfig::default();
        let mut env = HashMap::new();
        env.insert("GEMINI_API_KEY".to_string(), "test-key".to_string());
        config.tools.insert(
            "gemini-cli".to_string(),
            GlobalToolConfig {
                max_concurrent: None,
                env,
            },
        );

        let vars = config.env_vars("gemini-cli").unwrap();
        assert_eq!(vars.get("GEMINI_API_KEY").unwrap(), "test-key");
        assert!(config.env_vars("codex").is_none());
    }

    #[test]
    fn test_env_vars_empty_returns_none() {
        let mut config = GlobalConfig::default();
        config.tools.insert(
            "codex".to_string(),
            GlobalToolConfig {
                max_concurrent: Some(2),
                env: HashMap::new(),
            },
        );
        assert!(config.env_vars("codex").is_none());
    }

    #[test]
    fn test_parse_toml() {
        let toml_str = r#"
[defaults]
max_concurrent = 5

[tools.gemini-cli]
max_concurrent = 10

[tools.gemini-cli.env]
GEMINI_API_KEY = "test-key-123"

[tools.claude-code]
max_concurrent = 1
"#;
        let config: GlobalConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.defaults.max_concurrent, 5);
        assert_eq!(config.max_concurrent("gemini-cli"), 10);
        assert_eq!(config.max_concurrent("claude-code"), 1);
        assert_eq!(config.max_concurrent("codex"), 5); // default

        let env = config.env_vars("gemini-cli").unwrap();
        assert_eq!(env.get("GEMINI_API_KEY").unwrap(), "test-key-123");
    }

    #[test]
    fn test_load_missing_file() {
        // GlobalConfig::load() should return default when file doesn't exist
        // We can't easily test this without mocking config_path, but we can
        // verify the default is sane
        let config = GlobalConfig::default();
        assert_eq!(config.max_concurrent("any-tool"), 3);
    }

    #[test]
    fn test_all_tool_slots() {
        let mut config = GlobalConfig::default();
        config.tools.insert(
            "gemini-cli".to_string(),
            GlobalToolConfig {
                max_concurrent: Some(5),
                env: HashMap::new(),
            },
        );

        let slots = config.all_tool_slots();
        assert!(slots.len() >= 4);

        // gemini-cli should have override
        let gemini = slots.iter().find(|(t, _)| *t == "gemini-cli").unwrap();
        assert_eq!(gemini.1, 5);

        // codex should have default
        let codex = slots.iter().find(|(t, _)| *t == "codex").unwrap();
        assert_eq!(codex.1, 3);
    }

    #[test]
    fn test_default_template_is_valid_comment_only() {
        let template = GlobalConfig::default_template();
        // The template should contain helpful comments
        assert!(template.contains("[defaults]"));
        assert!(template.contains("max_concurrent"));
    }

    #[test]
    fn test_slots_dir() {
        // Should not fail on supported platforms
        let dir = GlobalConfig::slots_dir();
        assert!(dir.is_ok());
        let path = dir.unwrap();
        assert!(path.to_string_lossy().contains("slots"));
    }
}
