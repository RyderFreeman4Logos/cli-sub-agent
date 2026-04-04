use anyhow::{Result, bail};
use std::path::Path;

use crate::config::ProjectConfig;
use crate::global::ToolSelection;

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
    validate_acp(&config)?;
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
    if !config.resources.initial_estimates.is_empty() {
        tracing::warn!(
            "initial_estimates in [resources] is deprecated and will be ignored. \
             Memory scheduling no longer uses static per-tool estimates."
        );
    }
    if config.resources.idle_timeout_seconds == 0 {
        bail!("resources.idle_timeout_seconds must be > 0 (got 0)");
    }
    if config.resources.liveness_dead_seconds == Some(0) {
        bail!("resources.liveness_dead_seconds must be > 0 when set (got 0)");
    }
    if config.resources.slot_wait_timeout_seconds == 0 {
        bail!("resources.slot_wait_timeout_seconds must be > 0 (got 0)");
    }
    if config.resources.stdin_write_timeout_seconds == 0 {
        bail!("resources.stdin_write_timeout_seconds must be > 0 (got 0)");
    }
    if config.resources.termination_grace_period_seconds == 0 {
        bail!("resources.termination_grace_period_seconds must be > 0 (got 0)");
    }
    if let Some(mem) = config.resources.memory_max_mb
        && mem < 256
    {
        bail!(
            "resources.memory_max_mb must be >= 256 (got {mem}). \
                 Tool processes need at least 256 MB to function."
        );
    }
    if let Some(heap) = config.resources.node_heap_limit_mb
        && heap < 512
    {
        bail!(
            "resources.node_heap_limit_mb must be >= 512 (got {heap}). \
                 Node-based tools need at least 512 MB heap to function."
        );
    }
    if let Some(pids) = config.resources.pids_max
        && pids < 10
    {
        bail!(
            "resources.pids_max must be >= 10 (got {pids}). \
                 Tool processes need at least 10 PIDs for process trees."
        );
    }
    if let Some(percent) = config.resources.soft_limit_percent
        && (percent == 0 || percent > 100)
    {
        bail!(
            "resources.soft_limit_percent must be 1-100 (got {percent}). \
             0 silently disables the memory monitor."
        );
    }
    if let Some(interval) = config.resources.memory_monitor_interval_seconds
        && interval == 0
    {
        bail!(
            "resources.memory_monitor_interval_seconds must be >= 1 (got 0). \
             Zero interval causes a busy-polling loop."
        );
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

fn validate_acp(config: &ProjectConfig) -> Result<()> {
    if config.acp.init_timeout_seconds == 0 {
        bail!("acp.init_timeout_seconds must be > 0 (got 0)");
    }
    Ok(())
}

fn validate_tools(config: &ProjectConfig) -> Result<()> {
    let known_tools = [
        "gemini-cli",
        "opencode",
        "codex",
        "claude-code",
        "openai-compat",
    ];
    for (tool_name, tool_config) in &config.tools {
        if !known_tools.contains(&tool_name.as_str()) {
            bail!("Unknown tool '{tool_name}'. Known tools: {known_tools:?}");
        }
        // Validate per-tool sandbox memory overrides.
        if let Some(mem) = tool_config.memory_max_mb
            && mem < 256
        {
            bail!(
                "tools.{tool_name}.memory_max_mb must be >= 256 (got {mem}). \
                     Tool processes need at least 256 MB to function."
            );
        }
        if let Some(heap) = tool_config.node_heap_limit_mb
            && heap < 512
        {
            bail!(
                "tools.{tool_name}.node_heap_limit_mb must be >= 512 (got {heap}). \
                     Node-based tools need at least 512 MB heap to function."
            );
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
                    "tools.{tool_name}.enforcement_mode = \"required\" but no memory_max_mb is set \
                     (neither tools.{tool_name}.memory_max_mb nor resources.memory_max_mb). \
                     Required mode needs an explicit memory limit to enforce."
                );
            }
        }
    }
    Ok(())
}

/// Validate a `ToolSelection` value for review/debate config.
fn validate_tool_selection(tool: &ToolSelection, section: &str) -> Result<()> {
    let single_supported = ["auto", "gemini-cli", "opencode", "codex", "claude-code"];
    let whitelist_supported = ["gemini-cli", "opencode", "codex", "claude-code"];
    match tool {
        ToolSelection::Single(s) => {
            if !single_supported.contains(&s.as_str()) {
                bail!(
                    "Invalid [{section}].tool value '{s}'. \
                     Supported values: auto, gemini-cli, opencode, codex, claude-code."
                );
            }
        }
        ToolSelection::Whitelist(tools) => {
            for t in tools {
                if !whitelist_supported.contains(&t.as_str()) {
                    bail!(
                        "Invalid tool '{t}' in [{section}].tool array. \
                         Supported values: gemini-cli, opencode, codex, claude-code. \
                         ('auto' is not valid inside a whitelist array)"
                    );
                }
            }
        }
    }
    Ok(())
}

fn validate_review(config: &ProjectConfig) -> Result<()> {
    let Some(review) = &config.review else {
        return Ok(());
    };

    validate_tool_selection(&review.tool, "review")?;
    if let Some(tier_name) = &review.tier
        && !config.tiers.contains_key(tier_name)
    {
        bail!(
            "[review].tier references unknown tier '{}'. Available tiers: {:?}",
            tier_name,
            config.tiers.keys().collect::<Vec<_>>()
        );
    }
    Ok(())
}

fn validate_debate(config: &ProjectConfig) -> Result<()> {
    let Some(debate) = &config.debate else {
        return Ok(());
    };

    validate_tool_selection(&debate.tool, "debate")?;
    if let Some(tier_name) = &debate.tier
        && !config.tiers.contains_key(tier_name)
    {
        bail!(
            "[debate].tier references unknown tier '{}'. Available tiers: {:?}",
            tier_name,
            config.tiers.keys().collect::<Vec<_>>()
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
            bail!("Tier '{tier_name}' must have at least one model");
        }
        for model_spec in &tier_config.models {
            validate_model_spec(tier_name, model_spec)?;
        }
        // Validate budget constraints
        if let Some(budget) = tier_config.token_budget
            && budget == 0
        {
            bail!("Tier '{tier_name}': token_budget must be > 0 (got 0)");
        }
        if let Some(turns) = tier_config.max_turns
            && turns == 0
        {
            bail!("Tier '{tier_name}': max_turns must be > 0 (got 0)");
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
    let known_tools = [
        "gemini-cli",
        "opencode",
        "codex",
        "claude-code",
        "openai-compat",
    ];
    if let Some(prefs) = &config.preferences {
        for name in &prefs.tool_priority {
            if !known_tools.contains(&name.as_str()) {
                eprintln!(
                    "warning: Unrecognized tool in [preferences].tool_priority: '{name}'. \
                     Known tools: {known_tools:?}. Entry will be ignored for sorting."
                );
            }
        }
    }
}

fn validate_model_spec(tier_name: &str, model_spec: &str) -> Result<()> {
    let parts: Vec<&str> = model_spec.split('/').collect();
    if parts.len() != 4 {
        bail!(
            "Tier '{tier_name}' has invalid model spec '{model_spec}'. Expected format: 'tool/provider/model/budget'"
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

#[cfg(test)]
#[path = "validate_tests_tail.rs"]
mod tests_tail;
