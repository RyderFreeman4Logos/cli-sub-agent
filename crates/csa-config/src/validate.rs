use anyhow::{bail, Result};
use std::path::Path;

use crate::config::ProjectConfig;

/// Validate a project configuration file.
/// Returns Ok(()) if valid, or Err with descriptive messages.
pub fn validate_config(project_root: &Path) -> Result<()> {
    let config = ProjectConfig::load(project_root)?;
    let config = match config {
        Some(c) => c,
        None => bail!("No configuration found. Run `csa init` first."),
    };

    validate_project_meta(&config)?;
    validate_tools(&config)?;
    validate_tiers(&config)?;

    Ok(())
}

fn validate_project_meta(config: &ProjectConfig) -> Result<()> {
    if config.project.name.is_empty() {
        bail!("project.name cannot be empty");
    }
    if config.project.max_recursion_depth > 20 {
        bail!(
            "project.max_recursion_depth ({}) seems too high (max recommended: 20)",
            config.project.max_recursion_depth
        );
    }
    Ok(())
}

fn validate_tools(config: &ProjectConfig) -> Result<()> {
    let known_tools = ["gemini-cli", "opencode", "codex", "claude-code"];
    for tool_name in config.tools.keys() {
        if !known_tools.contains(&tool_name.as_str()) {
            bail!(
                "Unknown tool '{}'. Known tools: {:?}",
                tool_name,
                known_tools
            );
        }
    }
    Ok(())
}

fn validate_tiers(config: &ProjectConfig) -> Result<()> {
    let valid_tier_names = ["tier1", "tier2", "tier3", "tier4", "tier5"];
    for tier_name in config.tiers.keys() {
        if !valid_tier_names.contains(&tier_name.as_str()) {
            bail!(
                "Unknown tier '{}'. Valid tiers: {:?}",
                tier_name,
                valid_tier_names
            );
        }
    }
    // Validate tier_mapping values reference valid tiers
    for (task_type, tier_ref) in &config.tier_mapping {
        if !valid_tier_names.contains(&tier_ref.as_str()) {
            bail!(
                "tier_mapping.{} references unknown tier '{}'",
                task_type,
                tier_ref
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProjectConfig, ProjectMeta, ResourcesConfig, ToolConfig};
    use chrono::Utc;
    use std::collections::HashMap;
    use tempfile::tempdir;

    #[test]
    fn test_validate_config_succeeds_on_valid() {
        let dir = tempdir().unwrap();

        let mut tools = HashMap::new();
        tools.insert(
            "codex".to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
            },
        );

        let mut tier_mapping = HashMap::new();
        tier_mapping.insert("security_audit".to_string(), "tier1".to_string());

        let config = ProjectConfig {
            project: ProjectMeta {
                name: "test-project".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools,
            tiers: HashMap::new(),
            tier_mapping,
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();

        let result = validate_config(dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_config_fails_on_empty_name() {
        let dir = tempdir().unwrap();

        let config = ProjectConfig {
            project: ProjectMeta {
                name: "".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools: HashMap::new(),
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();

        let result = validate_config(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_validate_config_fails_on_unknown_tool() {
        let dir = tempdir().unwrap();

        let mut tools = HashMap::new();
        tools.insert(
            "unknown-tool".to_string(),
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

        config.save(dir.path()).unwrap();

        let result = validate_config(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown tool"));
    }

    #[test]
    fn test_validate_config_fails_on_invalid_tier_name() {
        let dir = tempdir().unwrap();

        let mut tiers = HashMap::new();
        tiers.insert("invalid-tier".to_string(), vec!["codex".to_string()]);

        let config = ProjectConfig {
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools: HashMap::new(),
            tiers,
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();

        let result = validate_config(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown tier"));
    }

    #[test]
    fn test_validate_config_fails_on_invalid_tier_mapping() {
        let dir = tempdir().unwrap();

        let mut tier_mapping = HashMap::new();
        tier_mapping.insert("security_audit".to_string(), "invalid-tier".to_string());

        let config = ProjectConfig {
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools: HashMap::new(),
            tiers: HashMap::new(),
            tier_mapping,
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();

        let result = validate_config(dir.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("tier_mapping") || err_msg.contains("unknown tier"));
    }

    #[test]
    fn test_validate_config_fails_if_no_config() {
        let dir = tempdir().unwrap();
        // No config created

        let result = validate_config(dir.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No configuration found"));
    }
}
