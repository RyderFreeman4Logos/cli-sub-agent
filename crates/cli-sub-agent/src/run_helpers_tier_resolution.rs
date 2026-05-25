use anyhow::Result;
use csa_config::ProjectConfig;
use csa_core::types::ToolName;

use super::{is_tool_binary_available_for_config, parse_tool_name};

#[derive(Debug, Clone)]
pub(crate) struct TierToolResolution {
    pub tool: ToolName,
    pub model_spec: String,
}

pub(crate) fn collect_available_tier_models(
    tier_name: &str,
    config: &ProjectConfig,
    whitelist: Option<&[String]>,
    skip_specs: &[String],
) -> Vec<TierToolResolution> {
    let Some(tier) = config.tiers.get(tier_name) else {
        return Vec::new();
    };

    tier.models
        .iter()
        .filter_map(|spec| {
            if skip_specs.iter().any(|s| s == spec) {
                return None;
            }
            let parts: Vec<&str> = spec.splitn(4, '/').collect();
            if parts.len() != 4 {
                return None;
            }
            let tool_str = parts[0];
            let tool = parse_tool_name(tool_str).ok()?;
            if !config.is_tool_enabled(tool_str)
                || !is_tool_binary_available_for_config(tool_str, Some(config))
            {
                return None;
            }
            if let Some(wl) = whitelist
                && !wl.iter().any(|w| w == tool_str)
            {
                return None;
            }
            Some(TierToolResolution {
                tool,
                model_spec: spec.clone(),
            })
        })
        .collect()
}

pub(crate) fn resolve_requested_tool_from_tier(
    tier_name: &str,
    config: &ProjectConfig,
    parent_tool: Option<&str>,
    requested_tool: ToolName,
    force_override_user_config: bool,
    skip_specs: &[String],
) -> Result<TierToolResolution> {
    let requested_tool_name = requested_tool.as_str();
    let Some(tier) = config.tiers.get(tier_name) else {
        anyhow::bail!("Tier '{}' not found.", tier_name);
    };
    let tool_in_tier = tier.models.iter().any(|spec| {
        !skip_specs.iter().any(|skip| skip == spec)
            && spec
                .split('/')
                .next()
                .is_some_and(|tool_name| tool_name == requested_tool_name)
    });
    if !tool_in_tier {
        let suggestions = config.suggest_compatible_alternatives(requested_tool_name, tier_name);
        anyhow::bail!(
            "Tool '{}' is not available in tier '{}'\n\n{}",
            requested_tool_name,
            tier_name,
            suggestions
        );
    }

    config.enforce_tool_enabled(requested_tool_name, force_override_user_config)?;
    let whitelist = [requested_tool_name.to_string()];
    if let Some(resolution) =
        resolve_tool_from_tier(tier_name, config, parent_tool, Some(&whitelist), skip_specs)
    {
        return Ok(resolution);
    }

    anyhow::bail!(
        "Requested tool '{}' is configured in tier '{}' but is not currently available. \
         Ensure it is installed and enabled.",
        requested_tool_name,
        tier_name
    );
}

pub(crate) fn resolve_tool_from_tier(
    tier_name: &str,
    config: &ProjectConfig,
    parent_tool: Option<&str>,
    whitelist: Option<&[String]>,
    skip_specs: &[String],
) -> Option<TierToolResolution> {
    let parent_family = parent_tool
        .and_then(|p| parse_tool_name(p).ok())
        .map(|t| t.model_family());
    let available = collect_available_tier_models(tier_name, config, whitelist, skip_specs);
    if available.is_empty() {
        return None;
    }
    if let Some(parent_fam) = parent_family
        && let Some(resolution) = available
            .iter()
            .find(|resolution| resolution.tool.model_family() != parent_fam)
    {
        return Some(resolution.clone());
    }
    Some(available[0].clone())
}
