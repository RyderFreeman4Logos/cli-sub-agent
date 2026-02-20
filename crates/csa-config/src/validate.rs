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
    warn_unknown_tool_priority(&config);

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
    if let Some(mem) = config.resources.memory_max_mb {
        if mem < 256 {
            bail!(
                "resources.memory_max_mb must be >= 256 (got {}). \
                 Tool processes need at least 256 MB to function.",
                mem
            );
        }
    }
    if let Some(heap) = config.resources.node_heap_limit_mb {
        if heap < 512 {
            bail!(
                "resources.node_heap_limit_mb must be >= 512 (got {}). \
                 Node-based tools need at least 512 MB heap to function.",
                heap
            );
        }
    }
    if let Some(pids) = config.resources.pids_max {
        if pids < 10 {
            bail!(
                "resources.pids_max must be >= 10 (got {}). \
                 Tool processes need at least 10 PIDs for process trees.",
                pids
            );
        }
    }
    // Required enforcement mode demands an explicit memory limit.
    if matches!(
        config.resources.enforcement_mode,
        Some(crate::config::EnforcementMode::Required)
    ) && config.resources.memory_max_mb.is_none()
    {
        bail!(
            "resources.enforcement_mode = \"required\" but resources.memory_max_mb is not set. \
             Required mode needs an explicit memory limit to enforce."
        );
    }
    Ok(())
}

fn validate_tools(config: &ProjectConfig) -> Result<()> {
    let known_tools = ["gemini-cli", "opencode", "codex", "claude-code"];
    for (tool_name, tool_config) in &config.tools {
        if !known_tools.contains(&tool_name.as_str()) {
            bail!(
                "Unknown tool '{}'. Known tools: {:?}",
                tool_name,
                known_tools
            );
        }
        // Validate per-tool sandbox memory overrides.
        if let Some(mem) = tool_config.memory_max_mb {
            if mem < 256 {
                bail!(
                    "tools.{}.memory_max_mb must be >= 256 (got {}). \
                     Tool processes need at least 256 MB to function.",
                    tool_name,
                    mem
                );
            }
        }
        if let Some(heap) = tool_config.node_heap_limit_mb {
            if heap < 512 {
                bail!(
                    "tools.{}.node_heap_limit_mb must be >= 512 (got {}). \
                     Node-based tools need at least 512 MB heap to function.",
                    tool_name,
                    heap
                );
            }
        }
        // Per-tool required enforcement demands a resolvable memory_max_mb.
        if matches!(
            tool_config.enforcement_mode,
            Some(crate::config::EnforcementMode::Required)
        ) {
            let has_memory =
                tool_config.memory_max_mb.is_some() || config.resources.memory_max_mb.is_some();
            if !has_memory {
                bail!(
                    "tools.{}.enforcement_mode = \"required\" but no memory_max_mb is set \
                     (neither tools.{0}.memory_max_mb nor resources.memory_max_mb). \
                     Required mode needs an explicit memory limit to enforce.",
                    tool_name
                );
            }
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

/// Warn (non-fatal) if `preferences.tool_priority` contains unrecognized tool names.
/// Unknown entries are harmless (sorted to end) but likely indicate a typo.
fn warn_unknown_tool_priority(config: &ProjectConfig) {
    let known_tools = ["gemini-cli", "opencode", "codex", "claude-code"];
    if let Some(prefs) = &config.preferences {
        for name in &prefs.tool_priority {
            if !known_tools.contains(&name.as_str()) {
                eprintln!(
                    "warning: Unrecognized tool in [preferences].tool_priority: '{}'. \
                     Known tools: {:?}. Entry will be ignored for sorting.",
                    name, known_tools
                );
            }
        }
    }
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

    // Validate tool name is a known tool
    let tool_part = parts[0];
    let known_tools: Vec<&str> = crate::global::all_known_tools()
        .iter()
        .map(|t| t.as_str())
        .collect();
    if !known_tools.contains(&tool_part) {
        bail!(
            "Tier '{}' has model spec '{}' with unknown tool '{}'. \
             Known tools: [{}].",
            tier_name,
            model_spec,
            tool_part,
            known_tools.join(", ")
        );
    }

    Ok(())
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
