use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierConfig {
    pub description: String,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub project: ProjectMeta,
    #[serde(default)]
    pub resources: ResourcesConfig,
    #[serde(default)]
    pub tools: HashMap<String, ToolConfig>,
    #[serde(default)]
    pub tiers: HashMap<String, TierConfig>,
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

    /// Resolve tier-based tool selection for a given task type.
    ///
    /// Returns (tool_name, model_spec_string) for the first enabled tool in the tier.
    /// Falls back to tier3 if task_type not found in tier_mapping.
    /// Returns None if no enabled tools found.
    pub fn resolve_tier_tool(&self, task_type: &str) -> Option<(String, String)> {
        // 1. Look up task_type in tier_mapping to get tier name
        let tier_name = self
            .tier_mapping
            .get(task_type)
            .map(String::as_str)
            .or_else(|| {
                // Fallback: try to find tier3 or tier-3-*
                if self.tiers.contains_key("tier3") {
                    Some("tier3")
                } else {
                    self.tiers
                        .keys()
                        .find(|k| k.starts_with("tier-3-") || k.starts_with("tier3"))
                        .map(String::as_str)
                }
            })?;

        // 2. Find that tier in tiers map
        let tier = self.tiers.get(tier_name)?;

        // 3. Iterate through tier's models (format: tool/provider/model/thinking_budget)
        for model_spec_str in &tier.models {
            // Parse model spec to extract tool name
            let parts: Vec<&str> = model_spec_str.splitn(4, '/').collect();
            if parts.len() != 4 {
                continue; // Invalid format, skip
            }

            let tool_name = parts[0];

            // 4. Check if this tool is enabled
            if self.is_tool_enabled(tool_name) {
                return Some((tool_name.to_string(), model_spec_str.clone()));
            }
        }

        None
    }

    /// Resolve alias to model spec string.
    ///
    /// If input is an alias key, returns the resolved value.
    /// Otherwise, returns the input unchanged.
    pub fn resolve_alias(&self, input: &str) -> String {
        self.aliases
            .get(input)
            .cloned()
            .unwrap_or_else(|| input.to_string())
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

    #[test]
    fn test_resolve_tier_default_selection() {
        let mut tools = HashMap::new();
        tools.insert(
            "gemini-cli".to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
            },
        );
        tools.insert(
            "codex".to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
            },
        );

        let mut tiers = HashMap::new();
        tiers.insert(
            "tier1".to_string(),
            TierConfig {
                description: "Quick tier".to_string(),
                models: vec![
                    "gemini-cli/google/gemini-3-flash-preview/xhigh".to_string(),
                    "codex/anthropic/claude-opus/high".to_string(),
                ],
            },
        );

        let mut tier_mapping = HashMap::new();
        tier_mapping.insert("default".to_string(), "tier1".to_string());

        let config = ProjectConfig {
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools,
            tiers,
            tier_mapping,
            aliases: HashMap::new(),
        };

        let result = config.resolve_tier_tool("default");
        assert!(result.is_some());
        let (tool_name, model_spec) = result.unwrap();
        assert_eq!(tool_name, "gemini-cli");
        assert_eq!(model_spec, "gemini-cli/google/gemini-3-flash-preview/xhigh");
    }

    #[test]
    fn test_resolve_tier_fallback_to_tier3() {
        let mut tools = HashMap::new();
        tools.insert(
            "codex".to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
            },
        );

        let mut tiers = HashMap::new();
        tiers.insert(
            "tier3".to_string(),
            TierConfig {
                description: "Fallback tier".to_string(),
                models: vec!["codex/anthropic/claude-opus/medium".to_string()],
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
            tiers,
            tier_mapping: HashMap::new(), // No mapping for "unknown_task"
            aliases: HashMap::new(),
        };

        // Should fallback to tier3
        let result = config.resolve_tier_tool("unknown_task");
        assert!(result.is_some());
        let (tool_name, model_spec) = result.unwrap();
        assert_eq!(tool_name, "codex");
        assert_eq!(model_spec, "codex/anthropic/claude-opus/medium");
    }

    #[test]
    fn test_resolve_tier_skips_disabled_tools() {
        let mut tools = HashMap::new();
        tools.insert(
            "gemini-cli".to_string(),
            ToolConfig {
                enabled: false, // Disabled
                restrictions: None,
            },
        );
        tools.insert(
            "codex".to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
            },
        );

        let mut tiers = HashMap::new();
        tiers.insert(
            "tier1".to_string(),
            TierConfig {
                description: "Test tier".to_string(),
                models: vec![
                    "gemini-cli/google/gemini-3-flash-preview/xhigh".to_string(),
                    "codex/anthropic/claude-opus/high".to_string(),
                ],
            },
        );

        let mut tier_mapping = HashMap::new();
        tier_mapping.insert("default".to_string(), "tier1".to_string());

        let config = ProjectConfig {
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools,
            tiers,
            tier_mapping,
            aliases: HashMap::new(),
        };

        // Should skip disabled gemini-cli and select codex
        let result = config.resolve_tier_tool("default");
        assert!(result.is_some());
        let (tool_name, _) = result.unwrap();
        assert_eq!(tool_name, "codex");
    }

    #[test]
    fn test_resolve_alias() {
        let mut aliases = HashMap::new();
        aliases.insert(
            "fast".to_string(),
            "gemini-cli/google/gemini-3-flash-preview/low".to_string(),
        );
        aliases.insert(
            "smart".to_string(),
            "codex/anthropic/claude-opus/xhigh".to_string(),
        );

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
            aliases,
        };

        // Resolve alias
        assert_eq!(
            config.resolve_alias("fast"),
            "gemini-cli/google/gemini-3-flash-preview/low"
        );
        assert_eq!(
            config.resolve_alias("smart"),
            "codex/anthropic/claude-opus/xhigh"
        );

        // Non-alias should be returned unchanged
        assert_eq!(
            config.resolve_alias("codex/anthropic/claude-opus/high"),
            "codex/anthropic/claude-opus/high"
        );
    }

    #[test]
    fn test_max_recursion_depth_override() {
        let dir = tempdir().unwrap();

        // Config with custom max_recursion_depth
        let config = ProjectConfig {
            project: ProjectMeta {
                name: "test-project".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 10,
            },
            resources: ResourcesConfig::default(),
            tools: HashMap::new(),
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();

        let loaded = ProjectConfig::load(dir.path()).unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();

        assert_eq!(loaded.project.max_recursion_depth, 10);
    }

    #[test]
    fn test_max_recursion_depth_default() {
        let dir = tempdir().unwrap();

        // Config without explicitly setting max_recursion_depth (should use default)
        let config_toml = r#"
[project]
name = "test-project"
created_at = "2024-01-01T00:00:00Z"

[resources]
"#;

        let config_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.toml"), config_toml).unwrap();

        let loaded = ProjectConfig::load(dir.path()).unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();

        assert_eq!(loaded.project.max_recursion_depth, 5);
    }

    #[test]
    #[ignore] // Only run manually to test actual project config
    fn test_load_actual_project_config() {
        // Try to find project root
        let current_dir = std::env::current_dir().unwrap();
        let mut project_root = current_dir.as_path();

        // Walk up until we find .csa/config.toml
        loop {
            let config_path = project_root.join(".csa/config.toml");
            if config_path.exists() {
                println!("Found config at: {}", config_path.display());
                break;
            }
            project_root = match project_root.parent() {
                Some(p) => p,
                None => {
                    println!("Could not find .csa/config.toml in parent directories");
                    return;
                }
            };
        }

        let result = ProjectConfig::load(project_root);
        assert!(result.is_ok(), "Failed to load config: {:?}", result.err());

        let config = result.unwrap();
        assert!(config.is_some(), "Config should exist");

        let config = config.unwrap();
        println!(
            "✓ Successfully loaded project config: {}",
            config.project.name
        );
        println!("✓ Tiers defined: {}", config.tiers.len());

        for (name, tier_config) in &config.tiers {
            println!(
                "  - {}: {} (models: {})",
                name,
                tier_config.description,
                tier_config.models.len()
            );
            assert!(
                !tier_config.models.is_empty(),
                "Tier {} should have models",
                name
            );
            for model in &tier_config.models {
                let parts: Vec<&str> = model.split('/').collect();
                assert_eq!(
                    parts.len(),
                    4,
                    "Model spec '{}' should have format 'tool/provider/model/budget'",
                    model
                );
            }
        }

        println!("✓ Tier mappings defined: {}", config.tier_mapping.len());
        for (task, tier) in &config.tier_mapping {
            println!("  - {} -> {}", task, tier);
            assert!(
                config.tiers.contains_key(tier),
                "Tier mapping {} references undefined tier {}",
                task,
                tier
            );
        }

        println!("✓ All validation checks passed!");
    }
}
