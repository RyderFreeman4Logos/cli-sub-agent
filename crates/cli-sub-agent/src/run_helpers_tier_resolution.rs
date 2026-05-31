use anyhow::Result;
use csa_config::ProjectConfig;
use csa_core::types::ToolName;

use super::{is_tool_binary_available_for_config, parse_tool_name};
use crate::failover_trace::{FailoverSkipKind, TierModelExclusion};

#[derive(Debug, Clone)]
pub(crate) struct TierToolResolution {
    pub tool: ToolName,
    pub model_spec: String,
}

/// Walk a tier's model list in definition order, partitioning each spec into
/// either an available [`TierToolResolution`] or a [`TierModelExclusion`] that
/// records WHY it was filtered out (#1714). [`collect_available_tier_models`]
/// returns only the `included` half; the `excluded` half feeds the failover
/// trace so the orchestrator can distinguish a disabled/undetected tool from a
/// quota-exhausted one.
///
/// `skip_specs` (already-attempted models in the current failover) are dropped
/// from both halves: they are tracked as attempt failures elsewhere, so
/// recording them here too would double-count them in the chain.
pub(crate) fn evaluate_tier_models(
    tier_name: &str,
    config: &ProjectConfig,
    whitelist: Option<&[String]>,
    skip_specs: &[String],
) -> (Vec<TierToolResolution>, Vec<TierModelExclusion>) {
    let mut included = Vec::new();
    let mut excluded = Vec::new();
    let Some(tier) = config.tiers.get(tier_name) else {
        return (included, excluded);
    };

    for spec in &tier.models {
        if skip_specs.iter().any(|s| s == spec) {
            continue;
        }
        let parts: Vec<&str> = spec.splitn(4, '/').collect();
        if parts.len() != 4 {
            excluded.push(TierModelExclusion {
                model_spec: spec.clone(),
                tool: None,
                kind: FailoverSkipKind::MalformedSpec,
            });
            continue;
        }
        let tool_str = parts[0];
        let Ok(tool) = parse_tool_name(tool_str) else {
            excluded.push(TierModelExclusion {
                model_spec: spec.clone(),
                tool: None,
                kind: FailoverSkipKind::MalformedSpec,
            });
            continue;
        };
        if !config.is_tool_enabled(tool_str) {
            excluded.push(TierModelExclusion {
                model_spec: spec.clone(),
                tool: Some(tool),
                kind: FailoverSkipKind::Disabled,
            });
            continue;
        }
        if !is_tool_binary_available_for_config(tool_str, Some(config)) {
            excluded.push(TierModelExclusion {
                model_spec: spec.clone(),
                tool: Some(tool),
                kind: FailoverSkipKind::AvailabilityDetectionMiss,
            });
            continue;
        }
        if let Some(wl) = whitelist
            && !wl.iter().any(|w| w == tool_str)
        {
            excluded.push(TierModelExclusion {
                model_spec: spec.clone(),
                tool: Some(tool),
                kind: FailoverSkipKind::WhitelistFiltered,
            });
            continue;
        }
        included.push(TierToolResolution {
            tool,
            model_spec: spec.clone(),
        });
    }

    (included, excluded)
}

/// Available tier models in definition order. Thin wrapper over
/// [`evaluate_tier_models`] that discards exclusion bookkeeping; behaviour is
/// identical to the pre-#1714 filter.
pub(crate) fn collect_available_tier_models(
    tier_name: &str,
    config: &ProjectConfig,
    whitelist: Option<&[String]>,
    skip_specs: &[String],
) -> Vec<TierToolResolution> {
    evaluate_tier_models(tier_name, config, whitelist, skip_specs).0
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
