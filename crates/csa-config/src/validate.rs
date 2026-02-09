use anyhow::{bail, Result};
use std::path::Path;

use crate::config::ProjectConfig;

/// Validate a project configuration file.
/// Returns Ok(()) if valid, or Err with descriptive messages.
pub fn validate_config(project_root: &Path) -> Result<()> {
    let config = ProjectConfig::load(project_root)?;
    validate_loaded_config(config)
}

/// Validate config loaded with explicit paths (bypasses user-level fallback).
/// Useful for testing without filesystem side effects.
#[cfg(test)]
pub(crate) fn validate_config_with_paths(
    user_path: Option<&Path>,
    project_path: &Path,
) -> Result<()> {
    let config = ProjectConfig::load_with_paths(user_path, project_path)?;
    validate_loaded_config(config)
}

fn validate_loaded_config(config: Option<ProjectConfig>) -> Result<()> {
    let config = match config {
        Some(c) => c,
        None => bail!("No configuration found. Run `csa init` first."),
    };

    validate_project_meta(&config)?;
    validate_tools(&config)?;
    validate_review(&config)?;
    validate_debate(&config)?;
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

fn validate_review(config: &ProjectConfig) -> Result<()> {
    let Some(review) = &config.review else {
        return Ok(());
    };

    let supported = ["auto", "gemini-cli", "opencode", "codex", "claude-code"];
    if !supported.contains(&review.tool.as_str()) {
        bail!(
            "Invalid [review].tool value '{}'. Supported values: auto, gemini-cli, opencode, codex, claude-code.",
            review.tool
        );
    }
    Ok(())
}

fn validate_debate(config: &ProjectConfig) -> Result<()> {
    let Some(debate) = &config.debate else {
        return Ok(());
    };

    let supported = ["auto", "gemini-cli", "opencode", "codex", "claude-code"];
    if !supported.contains(&debate.tool.as_str()) {
        bail!(
            "Invalid [debate].tool value '{}'. Supported values: auto, gemini-cli, opencode, codex, claude-code.",
            debate.tool
        );
    }
    Ok(())
}

fn validate_tiers(config: &ProjectConfig) -> Result<()> {
    // Validate tier names are non-empty
    for tier_name in config.tiers.keys() {
        if tier_name.is_empty() {
            bail!("Tier name cannot be empty");
        }
    }

    // Validate each TierConfig
    for (tier_name, tier_config) in &config.tiers {
        if tier_config.models.is_empty() {
            bail!("Tier '{}' must have at least one model", tier_name);
        }
        for model_spec in &tier_config.models {
            validate_model_spec(tier_name, model_spec)?;
        }
    }

    // Validate tier_mapping values reference tiers that exist in the tiers map
    for (task_type, tier_ref) in &config.tier_mapping {
        if !config.tiers.contains_key(tier_ref) {
            bail!(
                "tier_mapping.{} references unknown tier '{}'. Available tiers: {:?}",
                task_type,
                tier_ref,
                config.tiers.keys().collect::<Vec<_>>()
            );
        }
    }
    Ok(())
}

fn validate_model_spec(tier_name: &str, model_spec: &str) -> Result<()> {
    let parts: Vec<&str> = model_spec.split('/').collect();
    if parts.len() != 4 {
        bail!(
            "Tier '{}' has invalid model spec '{}'. Expected format: 'tool/provider/model/budget'",
            tier_name,
            model_spec
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, ToolConfig, CURRENT_SCHEMA_VERSION,
    };
    use crate::global::ReviewConfig;
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
                suppress_notify: false,
            },
        );

        let mut tiers = HashMap::new();
        tiers.insert(
            "tier-1-quick".to_string(),
            TierConfig {
                description: "Quick tasks".to_string(),
                models: vec!["gemini-cli/google/gemini-3-flash-preview/xhigh".to_string()],
            },
        );

        let mut tier_mapping = HashMap::new();
        tier_mapping.insert("security_audit".to_string(), "tier-1-quick".to_string());

        let config = ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test-project".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools,
            review: None,
            debate: None,
            tiers,
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
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools: HashMap::new(),
            review: None,
            debate: None,
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
                suppress_notify: false,
            },
        );

        let config = ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools,
            review: None,
            debate: None,
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
    fn test_validate_config_fails_on_invalid_review_tool() {
        let dir = tempdir().unwrap();

        let config = ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools: HashMap::new(),
            review: Some(ReviewConfig {
                tool: "invalid-tool".to_string(),
            }),
            debate: None,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();

        let result = validate_config(dir.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid [review].tool value"));
    }

    #[test]
    fn test_validate_config_fails_on_invalid_model_spec() {
        let dir = tempdir().unwrap();

        let mut tiers = HashMap::new();
        tiers.insert(
            "test-tier".to_string(),
            TierConfig {
                description: "Test tier".to_string(),
                models: vec!["invalid-model-spec".to_string()],
            },
        );

        let config = ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools: HashMap::new(),
            review: None,
            debate: None,
            tiers,
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();

        let result = validate_config(dir.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid model spec"));
    }

    #[test]
    fn test_validate_config_fails_on_invalid_tier_mapping() {
        let dir = tempdir().unwrap();

        let mut tiers = HashMap::new();
        tiers.insert(
            "tier-1-quick".to_string(),
            TierConfig {
                description: "Quick tasks".to_string(),
                models: vec!["gemini-cli/google/gemini-3-flash-preview/xhigh".to_string()],
            },
        );

        let mut tier_mapping = HashMap::new();
        tier_mapping.insert("security_audit".to_string(), "nonexistent-tier".to_string());

        let config = ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools: HashMap::new(),
            review: None,
            debate: None,
            tiers,
            tier_mapping,
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();

        let result = validate_config(dir.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("tier_mapping") && err_msg.contains("unknown tier"));
    }

    #[test]
    fn test_validate_config_fails_if_no_config() {
        let dir = tempdir().unwrap();
        // Use validate_config_with_paths(None, ...) to bypass user-level
        // fallback AND exercise the full validation path (None → bail!).
        let project_path = dir.path().join(".csa").join("config.toml");
        let result = validate_config_with_paths(None, &project_path);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No configuration found"));
    }

    #[test]
    fn test_validate_config_fails_on_empty_models() {
        let dir = tempdir().unwrap();

        let mut tiers = HashMap::new();
        tiers.insert(
            "empty-tier".to_string(),
            TierConfig {
                description: "Empty tier".to_string(),
                models: vec![],
            },
        );

        let config = ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools: HashMap::new(),
            review: None,
            debate: None,
            tiers,
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();

        let result = validate_config(dir.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must have at least one model"));
    }

    #[test]
    fn test_validate_config_accepts_custom_tier_names() {
        let dir = tempdir().unwrap();

        let mut tiers = HashMap::new();
        tiers.insert(
            "my-custom-tier".to_string(),
            TierConfig {
                description: "Custom tier name".to_string(),
                models: vec!["gemini-cli/google/gemini-3-flash-preview/xhigh".to_string()],
            },
        );

        let mut tier_mapping = HashMap::new();
        tier_mapping.insert("analysis".to_string(), "my-custom-tier".to_string());

        let config = ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools: HashMap::new(),
            review: None,
            debate: None,
            tiers,
            tier_mapping,
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();

        let result = validate_config(dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_config_fails_on_invalid_debate_tool() {
        let dir = tempdir().unwrap();

        let config = ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            tools: HashMap::new(),
            review: None,
            debate: Some(ReviewConfig {
                tool: "invalid-tool".to_string(),
            }),
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();

        let result = validate_config(dir.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid [debate].tool value"));
    }
}
