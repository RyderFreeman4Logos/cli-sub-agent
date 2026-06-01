use anyhow::Result;
use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use tracing::warn;

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
    skip_specs: &[String],
) -> Vec<TierToolResolution> {
    evaluate_tier_models(tier_name, config, skip_specs).0
}

pub(crate) fn collect_preferred_tier_models(
    tier_name: &str,
    config: &ProjectConfig,
    preference_order: &[String],
    skip_specs: &[String],
) -> Vec<TierToolResolution> {
    let available = collect_available_tier_models(tier_name, config, skip_specs);
    if preference_order.is_empty() {
        return available;
    }

    for preferred_tool in preference_order {
        if !available
            .iter()
            .any(|resolution| resolution.tool.as_str() == preferred_tool)
        {
            warn!(
                tier = tier_name,
                tool = preferred_tool,
                "Preferred tier tool is not available; continuing with the full tier candidate list"
            );
        }
    }

    let mut preferred = Vec::new();
    let mut remaining = Vec::new();
    for resolution in available {
        if preference_order
            .iter()
            .any(|preferred_tool| preferred_tool == resolution.tool.as_str())
        {
            preferred.push(resolution);
        } else {
            remaining.push(resolution);
        }
    }
    preferred.extend(remaining);
    preferred
}

pub(crate) fn resolve_preferred_tool_from_tier(
    tier_name: &str,
    config: &ProjectConfig,
    parent_tool: Option<&str>,
    preference_order: &[String],
    skip_specs: &[String],
) -> Result<TierToolResolution> {
    let Some(tier) = config.tiers.get(tier_name) else {
        anyhow::bail!("Tier '{}' not found.", tier_name);
    };

    for preferred_tool in preference_order {
        if !tier.models.iter().any(|spec| {
            !skip_specs.iter().any(|skip| skip == spec)
                && spec
                    .split('/')
                    .next()
                    .is_some_and(|tool_name| tool_name == preferred_tool)
        }) {
            let suggestions = config.suggest_compatible_alternatives(preferred_tool, tier_name);
            warn!(
                tier = tier_name,
                tool = preferred_tool,
                suggestions = %suggestions,
                "Preferred tool is not configured in tier; ignoring preference"
            );
        }
    }

    if let Some(resolution) =
        resolve_tool_from_tier(tier_name, config, parent_tool, preference_order, skip_specs)
    {
        return Ok(resolution);
    }

    anyhow::bail!(
        "Tier '{}' has no currently available tools. Ensure at least one tier tool is installed and enabled.",
        tier_name
    );
}

pub(crate) fn resolve_tool_from_tier(
    tier_name: &str,
    config: &ProjectConfig,
    parent_tool: Option<&str>,
    preference_order: &[String],
    skip_specs: &[String],
) -> Option<TierToolResolution> {
    let parent_family = parent_tool
        .and_then(|p| parse_tool_name(p).ok())
        .map(|t| t.model_family());
    let available = collect_preferred_tier_models(tier_name, config, preference_order, skip_specs);
    if available.is_empty() {
        return None;
    }
    if !preference_order.is_empty() {
        return Some(available[0].clone());
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
