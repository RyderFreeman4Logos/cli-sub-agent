use anyhow::{Result, bail};
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
    validate_resources(&config)?;
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

fn validate_resources(config: &ProjectConfig) -> Result<()> {
    if config.resources.idle_timeout_seconds == 0 {
        bail!("resources.idle_timeout_seconds must be > 0 (got 0)");
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
        // Validate budget constraints
        if let Some(budget) = tier_config.token_budget {
            if budget == 0 {
                bail!("Tier '{}': token_budget must be > 0 (got 0)", tier_name);
            }
        }
        if let Some(turns) = tier_config.max_turns {
            if turns == 0 {
                bail!("Tier '{}': max_turns must be > 0 (got 0)", tier_name);
            }
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
#[path = "validate_tests.rs"]
mod tests;
