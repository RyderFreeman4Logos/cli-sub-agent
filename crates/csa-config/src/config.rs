use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub project: ProjectMeta,
    #[serde(default)]
    pub resources: ResourcesConfig,
    #[serde(default)]
    pub tools: HashMap<String, ToolConfig>,
    #[serde(default)]
    pub tiers: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub tier_mapping: HashMap<String, String>,
    #[serde(default)]
    pub aliases: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    pub created_at: DateTime<Utc>,
    #[serde(default = "default_recursion_depth")]
    pub max_recursion_depth: u32,
}

fn default_recursion_depth() -> u32 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub restrictions: Option<ToolRestrictions>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRestrictions {
    #[serde(default)]
    pub allow_edit_existing_files: bool,
    #[serde(default)]
    pub allowed_operations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesConfig {
    #[serde(default = "default_min_mem")]
    pub min_free_memory_mb: u64,
    #[serde(default = "default_min_swap")]
    pub min_free_swap_mb: u64,
    #[serde(default)]
    pub initial_estimates: HashMap<String, u64>,
}

fn default_min_mem() -> u64 {
    2048
}

fn default_min_swap() -> u64 {
    1024
}

impl Default for ResourcesConfig {
    fn default() -> Self {
        Self {
            min_free_memory_mb: default_min_mem(),
            min_free_swap_mb: default_min_swap(),
            initial_estimates: HashMap::new(),
        }
    }
}

impl ProjectConfig {
    /// Load config from .csa/config.toml relative to project root.
    /// Returns None if config file doesn't exist (project not initialized).
    pub fn load(project_root: &Path) -> Result<Option<Self>> {
        let config_path = project_root.join(".csa").join("config.toml");
        if !config_path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&config_path)?;
        let config: ProjectConfig = toml::from_str(&content)?;
        Ok(Some(config))
    }

    /// Save config to .csa/config.toml
    pub fn save(&self, project_root: &Path) -> Result<()> {
        let config_dir = project_root.join(".csa");
        std::fs::create_dir_all(&config_dir)?;
        let config_path = config_dir.join("config.toml");
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, content)?;
        Ok(())
    }

    /// Check if a tool is enabled (unconfigured tools default to enabled)
    pub fn is_tool_enabled(&self, tool: &str) -> bool {
        self.tools.get(tool).map(|t| t.enabled).unwrap_or(true)
    }

    /// Check if a tool is allowed to edit existing files
    pub fn can_tool_edit_existing(&self, tool: &str) -> bool {
        self.tools
            .get(tool)
            .and_then(|t| t.restrictions.as_ref())
            .map(|r| r.allow_edit_existing_files)
            .unwrap_or(true) // Default: allow (only gemini-cli needs explicit deny)
    }

    /// Get the config file path for a project root
    pub fn config_path(project_root: &Path) -> std::path::PathBuf {
        project_root.join(".csa").join("config.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_load_nonexistent_returns_none() {
        let dir = tempdir().unwrap();
        let result = ProjectConfig::load(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = tempdir().unwrap();

        let mut tools = HashMap::new();
        tools.insert(
            "gemini-cli".to_string(),
            ToolConfig {
                enabled: true,
                restrictions: Some(ToolRestrictions {
                    allow_edit_existing_files: false,
                    allowed_operations: vec!["read".to_string(), "analyze".to_string()],
                }),
            },
        );

        let config = ProjectConfig {
            project: ProjectMeta {
                name: "test-project".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();

        let loaded = ProjectConfig::load(dir.path()).unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();

        assert_eq!(loaded.project.name, "test-project");
        assert_eq!(loaded.project.max_recursion_depth, 5);
        assert!(loaded.tools.contains_key("gemini-cli"));
        assert!(loaded.tools.get("gemini-cli").unwrap().enabled);
    }

    #[test]
    fn test_is_tool_enabled_configured_enabled() {
        let mut tools = HashMap::new();
        tools.insert(
            "codex".to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
            },
        );

        let config = ProjectConfig {
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        assert!(config.is_tool_enabled("codex"));
    }

    #[test]
    fn test_is_tool_enabled_configured_disabled() {
        let mut tools = HashMap::new();
        tools.insert(
            "codex".to_string(),
            ToolConfig {
                enabled: false,
                restrictions: None,
            },
        );

        let config = ProjectConfig {
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        assert!(!config.is_tool_enabled("codex"));
    }

    #[test]
    fn test_is_tool_enabled_unconfigured_defaults_to_true() {
        let config = ProjectConfig {
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools: HashMap::new(),
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        assert!(config.is_tool_enabled("codex"));
    }

    #[test]
    fn test_can_tool_edit_existing_with_restrictions_false() {
        let mut tools = HashMap::new();
        tools.insert(
            "gemini-cli".to_string(),
            ToolConfig {
                enabled: true,
                restrictions: Some(ToolRestrictions {
                    allow_edit_existing_files: false,
                    allowed_operations: vec![],
                }),
            },
        );

        let config = ProjectConfig {
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        assert!(!config.can_tool_edit_existing("gemini-cli"));
    }

    #[test]
    fn test_can_tool_edit_existing_without_restrictions() {
        let mut tools = HashMap::new();
        tools.insert(
            "codex".to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
            },
        );

        let config = ProjectConfig {
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        assert!(config.can_tool_edit_existing("codex"));
    }

    #[test]
    fn test_can_tool_edit_existing_unconfigured_defaults_to_true() {
        let config = ProjectConfig {
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools: HashMap::new(),
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        assert!(config.can_tool_edit_existing("codex"));
    }
}
