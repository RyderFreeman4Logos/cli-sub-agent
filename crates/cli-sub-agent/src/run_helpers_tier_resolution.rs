use anyhow::Result;
use csa_config::{ExecutionEnvOptions, GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_executor::{ModelSpec, model_spec::ModelSpecValidationError};
use tracing::warn;

use super::{is_tool_runtime_available_for_config_with_env, parse_tool_name};
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
    evaluate_tier_models_with_global_config(tier_name, config, None, skip_specs)
}

pub(crate) fn evaluate_tier_models_with_global_config(
    tier_name: &str,
    config: &ProjectConfig,
    global_config: Option<&GlobalConfig>,
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
        if let Err(kind) = validate_tier_model_spec_compatibility(spec) {
            excluded.push(TierModelExclusion {
                model_spec: spec.clone(),
                tool: Some(tool),
                kind,
            });
            continue;
        }
        if !config.is_tool_enabled(tool_str) {
            excluded.push(TierModelExclusion {
                model_spec: spec.clone(),
                tool: Some(tool),
                kind: FailoverSkipKind::Disabled,
            });
            continue;
        }
        let extra_env = global_config
            .and_then(|cfg| cfg.build_execution_env(tool_str, ExecutionEnvOptions::default()));
        if !is_tool_runtime_available_for_config_with_env(
            tool_str,
            Some(config),
            Some(spec),
            extra_env.as_ref(),
        ) {
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

pub(crate) fn validate_tier_model_spec_compatibility(
    spec: &str,
) -> std::result::Result<(), FailoverSkipKind> {
    let parsed = ModelSpec::parse(spec).map_err(|_| FailoverSkipKind::MalformedSpec)?;
    let known_tools: Vec<&'static str> = csa_config::global::all_known_tools()
        .iter()
        .map(|tool| tool.as_str())
        .collect();
    match parsed.validate_with_catalog(&known_tools) {
        Ok(()) => Ok(()),
        Err(ModelSpecValidationError::UnknownProvider { .. })
        | Err(ModelSpecValidationError::UnknownModel { .. }) => {
            Err(FailoverSkipKind::IncompatibleModel)
        }
        Err(ModelSpecValidationError::UnknownTool { .. }) => Err(FailoverSkipKind::MalformedSpec),
    }
}

/// Available tier models in definition order. Thin wrapper over
/// [`evaluate_tier_models`] that discards exclusion bookkeeping; behaviour is
/// identical to the pre-#1714 filter.
pub(crate) fn collect_available_tier_models_with_global_config(
    tier_name: &str,
    config: &ProjectConfig,
    global_config: Option<&GlobalConfig>,
    skip_specs: &[String],
) -> Vec<TierToolResolution> {
    evaluate_tier_models_with_global_config(tier_name, config, global_config, skip_specs).0
}

pub(crate) fn resolve_runtime_available_tier_fallback_with_global_config(
    config: &ProjectConfig,
    global_config: Option<&GlobalConfig>,
    task_type: &str,
    needs_edit: bool,
) -> Option<TierToolResolution> {
    let tier_name = config.resolve_tier_name_for_task(task_type)?;
    collect_available_tier_models_with_global_config(tier_name, config, global_config, &[])
        .into_iter()
        .find(|resolution| !needs_edit || config.is_tool_write_capable(resolution.tool.as_str()))
}

#[cfg(test)]
pub(crate) fn collect_preferred_tier_models(
    tier_name: &str,
    config: &ProjectConfig,
    preference_order: &[String],
    skip_specs: &[String],
) -> Vec<TierToolResolution> {
    collect_preferred_tier_models_with_global_config(
        tier_name,
        config,
        None,
        preference_order,
        skip_specs,
    )
}

pub(crate) fn collect_preferred_tier_models_with_global_config(
    tier_name: &str,
    config: &ProjectConfig,
    global_config: Option<&GlobalConfig>,
    preference_order: &[String],
    skip_specs: &[String],
) -> Vec<TierToolResolution> {
    let available = collect_available_tier_models_with_global_config(
        tier_name,
        config,
        global_config,
        skip_specs,
    );
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

    let mut ordered = Vec::new();
    let mut remaining = available;
    for preferred_tool in preference_order {
        let mut next_remaining = Vec::new();
        for resolution in remaining {
            if resolution.tool.as_str() == preferred_tool {
                ordered.push(resolution);
            } else {
                next_remaining.push(resolution);
            }
        }
        remaining = next_remaining;
    }
    ordered.extend(remaining);
    ordered
}

/// Resolve a tool/model from `tier_name`, honoring an explicitly-pinned `--tool`
/// preference (`preference_order`) as a SOFT reorder of the tier's ENABLED
/// candidates (#1749).
///
/// This function is only ever reached with an explicit user `--tool` pin: the
/// `run`, `review`, and `debate` paths each route config-derived preferences
/// through [`resolve_tool_from_tier`] instead and only call this helper when the
/// caller named a tool on the command line. Because the preference is an
/// explicit pin, a pin naming a configured-but-DISABLED tier candidate
/// (`[tools.<name>].enabled = false`) cannot be honored and FAILS FAST (#1836)
/// rather than silently substituting the tier default — a silent substitution
/// would also violate the `--no-failover` contract by running a different tool
/// than the one pinned. A pin naming a tool that is NOT a tier candidate also
/// FAILS FAST (#1994) — silently proceeding with a different tool caused the
/// operator to believe their preferred tool was running when it was not.
#[cfg(test)]
pub(crate) fn resolve_preferred_tool_from_tier(
    tier_name: &str,
    config: &ProjectConfig,
    parent_tool: Option<&str>,
    preference_order: &[String],
    skip_specs: &[String],
) -> Result<TierToolResolution> {
    resolve_preferred_tool_from_tier_with_global_config(
        tier_name,
        config,
        None,
        parent_tool,
        preference_order,
        skip_specs,
    )
}

pub(crate) fn resolve_preferred_tool_from_tier_with_global_config(
    tier_name: &str,
    config: &ProjectConfig,
    global_config: Option<&GlobalConfig>,
    parent_tool: Option<&str>,
    preference_order: &[String],
    skip_specs: &[String],
) -> Result<TierToolResolution> {
    if !config.tiers.contains_key(tier_name) {
        anyhow::bail!("Tier '{}' not found.", tier_name);
    }
    let (available, excluded) =
        evaluate_tier_models_with_global_config(tier_name, config, global_config, skip_specs);

    // #1836: a `--tool` pin that names a disabled tier candidate must fail
    // fast. Honoring the pin is impossible (the tool is gated off), so error
    // instead of falling through to the tier default. Enabled candidates are
    // handled by the soft-reorder below (#1749); non-candidates are rejected by
    // the next loop with a tier-candidate error (#1994).
    for preferred_tool in preference_order {
        let is_enabled_candidate = available
            .iter()
            .any(|resolution| resolution.tool.as_str() == preferred_tool.as_str());
        if is_enabled_candidate {
            continue;
        }
        let is_disabled_candidate = excluded.iter().any(|exclusion| {
            exclusion.kind == FailoverSkipKind::Disabled
                && exclusion
                    .tool
                    .is_some_and(|tool| tool.as_str() == preferred_tool.as_str())
        });
        if is_disabled_candidate {
            anyhow::bail!(
                "--tool {preferred_tool} requested but [tools.{preferred_tool}].enabled = false; \
                 enable it (config) or choose an enabled tool"
            );
        }
    }

    for preferred_tool in preference_order {
        if let Some(warning) =
            ignored_tier_tool_preference_warning(tier_name, preferred_tool, &available)
        {
            let suggestions = config.suggest_compatible_alternatives(preferred_tool, tier_name);
            anyhow::bail!(
                "--tool {preferred_tool} is not a candidate of tier '{tier_name}' \
                 (candidates: {suggestions}). Either add a {preferred_tool} model to the \
                 tier's models list or use --force-ignore-tier-setting to bypass tier routing. \
                 Detail: {warning}"
            );
        }
    }

    if let Some(resolution) = resolve_tool_from_tier_with_global_config(
        tier_name,
        config,
        global_config,
        parent_tool,
        preference_order,
        skip_specs,
    ) {
        return Ok(resolution);
    }

    anyhow::bail!(
        "Tier '{}' has no currently available tools. Ensure at least one tier tool is installed and enabled.",
        tier_name
    );
}

fn ignored_tier_tool_preference_warning(
    tier_name: &str,
    preferred_tool: &str,
    available: &[TierToolResolution],
) -> Option<String> {
    if available
        .iter()
        .any(|resolution| resolution.tool.as_str() == preferred_tool)
    {
        return None;
    }

    let mut candidate_tools: Vec<&str> = Vec::new();
    for resolution in available {
        let tool = resolution.tool.as_str();
        if !candidate_tools.contains(&tool) {
            candidate_tools.push(tool);
        }
    }

    let candidates = if candidate_tools.is_empty() {
        "none".to_string()
    } else {
        candidate_tools.join(", ")
    };
    let proceeding = available
        .first()
        .map(|resolution| resolution.tool.as_str())
        .unwrap_or("no available tool");

    Some(format!(
        "warning: --tool {preferred_tool} ignored - not an enabled candidate of tier '{tier_name}' (candidates: {candidates}); proceeding with {proceeding}"
    ))
}

#[cfg(test)]
pub(crate) fn resolve_tool_from_tier(
    tier_name: &str,
    config: &ProjectConfig,
    parent_tool: Option<&str>,
    preference_order: &[String],
    skip_specs: &[String],
) -> Option<TierToolResolution> {
    resolve_tool_from_tier_with_global_config(
        tier_name,
        config,
        None,
        parent_tool,
        preference_order,
        skip_specs,
    )
}

pub(crate) fn resolve_tool_from_tier_with_global_config(
    tier_name: &str,
    config: &ProjectConfig,
    global_config: Option<&GlobalConfig>,
    parent_tool: Option<&str>,
    preference_order: &[String],
    skip_specs: &[String],
) -> Option<TierToolResolution> {
    let parent_family = parent_tool
        .and_then(|p| parse_tool_name(p).ok())
        .map(|t| t.model_family());
    let available = collect_preferred_tier_models_with_global_config(
        tier_name,
        config,
        global_config,
        preference_order,
        skip_specs,
    );
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

#[cfg(test)]
mod tests {
    use super::{TierToolResolution, ignored_tier_tool_preference_warning};
    use csa_core::types::ToolName;

    #[test]
    fn ignored_tier_tool_preference_warning_names_candidates() {
        let warning = ignored_tier_tool_preference_warning(
            "tier-4-critical",
            "claude-code",
            &[
                TierToolResolution {
                    tool: ToolName::GeminiCli,
                    model_spec: "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
                },
                TierToolResolution {
                    tool: ToolName::Codex,
                    model_spec: "codex/openai/gpt-5.5/xhigh".to_string(),
                },
            ],
        )
        .expect("missing preferred tool should produce warning");

        assert!(warning.starts_with("warning:"), "{warning}");
        assert!(warning.contains("--tool claude-code ignored"), "{warning}");
        assert!(warning.contains("tier 'tier-4-critical'"), "{warning}");
        assert!(
            warning.contains("candidates: gemini-cli, codex"),
            "{warning}"
        );
        assert!(warning.contains("proceeding with gemini-cli"), "{warning}");
    }

    #[test]
    fn ignored_tier_tool_preference_warning_skips_available_tool() {
        let warning = ignored_tier_tool_preference_warning(
            "tier-4-critical",
            "codex",
            &[TierToolResolution {
                tool: ToolName::Codex,
                model_spec: "codex/openai/gpt-5.5/xhigh".to_string(),
            }],
        );

        assert_eq!(warning, None);
    }

    #[test]
    fn resolve_preferred_tool_from_tier_rejects_non_candidate() {
        use super::resolve_preferred_tool_from_tier;
        use crate::review_cmd::tests::project_config_with_enabled_tools;
        use csa_config::{TierStrategy, config::TierConfig};

        let mut config = project_config_with_enabled_tools(&["gemini-cli"]);
        config.tiers.insert(
            "test-tier".to_string(),
            TierConfig {
                description: "test".to_string(),
                models: vec!["gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string()],
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        );

        let err = resolve_preferred_tool_from_tier(
            "test-tier",
            &config,
            None,
            &["openai-compat".to_string()],
            &[],
        )
        .expect_err("--tool openai-compat must fail when not in tier candidates (#1994)");

        let msg = err.to_string();
        assert!(
            msg.contains("openai-compat") && msg.contains("not a candidate"),
            "error should name the rejected tool and explain why: {msg}"
        );
    }
}
