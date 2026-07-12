use std::path::Path;

use anyhow::{Context, Result};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedReviewSelection {
    pub(crate) tool: ToolName,
    pub(crate) model_spec: Option<String>,
    pub(crate) tier_preference_order: Vec<String>,
}

fn force_resolve_review_tool_from_tier(
    tier: &str,
    config: &ProjectConfig,
    global_config: &GlobalConfig,
    model_catalog: &csa_config::EffectiveModelCatalog,
    tool: ToolName,
) -> Option<crate::run_helpers::TierToolResolution> {
    let tier_config = config.tiers.get(tier)?;
    tier_config.models.iter().find_map(|model_spec| {
        let parts: Vec<&str> = model_spec.splitn(4, '/').collect();
        if parts.len() != 4 || parts[0] != tool.as_str() {
            return None;
        }
        if crate::run_helpers::validate_tier_model_spec_compatibility_with_catalog(
            model_spec,
            model_catalog,
        )
        .is_err()
        {
            return None;
        }
        let extra_env = global_config
            .build_execution_env(tool.as_str(), csa_config::ExecutionEnvOptions::default());
        if !crate::run_helpers::is_tool_runtime_available_for_config_with_env(
            tool.as_str(),
            Some(config),
            Some(model_spec),
            extra_env.as_ref(),
        ) {
            return None;
        }
        Some(crate::run_helpers::TierToolResolution {
            tool,
            model_spec: model_spec.clone(),
        })
    })
}

pub(crate) fn validate_review_direct_tool_tier_restriction(
    direct_tool_requested: bool,
    project_config: Option<&ProjectConfig>,
    effective_tier: Option<&str>,
    force_override_user_config: bool,
    force_ignore_tier_setting: bool,
    model_spec_provided: bool,
) -> Result<()> {
    crate::run_helpers::validate_direct_tool_tier_restriction(
        direct_tool_requested,
        project_config,
        effective_tier,
        force_override_user_config,
        force_ignore_tier_setting,
        model_spec_provided,
    )
}

/// Returns (tool, optional_model_spec). When tier resolves, model_spec is set.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_review_selection_with_catalog(
    arg_tool: Option<ToolName>,
    arg_model_spec: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    model_catalog: &csa_config::EffectiveModelCatalog,
    parent_tool: Option<&str>,
    project_root: &Path,
    force_override_user_config: bool,
    cli_tier: Option<&str>,
    force_ignore_tier_setting: bool,
    direct_tool_requested: bool,
) -> Result<ResolvedReviewSelection> {
    crate::run_helpers::validate_tool_tier_override_flags(
        direct_tool_requested,
        cli_tier,
        force_ignore_tier_setting,
    )?;
    crate::run_helpers::validate_model_spec_tier_conflict(arg_model_spec, cli_tier, "review")?;

    if let Some(model_spec) = arg_model_spec {
        let (tool, resolved_model_spec, _) =
            crate::run_helpers::resolve_tool_and_model(crate::run_helpers::RoutingRequest {
                tool: arg_tool,
                model_spec: Some(model_spec),
                model: None,
                thinking: None, // thinking not relevant for review command
                config: project_config,
                global_config: Some(global_config),
                model_catalog: Some(model_catalog),
                project_root,
                force: false,
                force_override_user_config,
                needs_edit: false,
                tier: cli_tier,
                force_ignore_tier_setting,
                tier_bypass_allowed: crate::run_helpers::tier_bypass_allowed(
                    project_config,
                    global_config,
                    false,
                ),
                tool_is_auto_resolved: false,
            })?;
        return Ok(ResolvedReviewSelection {
            tool,
            model_spec: resolved_model_spec,
            tier_preference_order: Vec::new(),
        });
    }

    // Enforce tier routing: block direct --tool when tiers are configured,
    // unless --force-ignore-tier-setting (or --force-override-user-config) is active.
    validate_review_direct_tool_tier_restriction(
        direct_tool_requested,
        project_config,
        cli_tier,
        force_override_user_config,
        force_ignore_tier_setting,
        arg_model_spec.is_some(),
    )?;

    let tier_name = super::resolve_review_tier_name(
        project_config,
        global_config,
        cli_tier,
        force_override_user_config,
        force_ignore_tier_setting,
    )?;

    if let Some(tool) = arg_tool {
        if let Some(ref tier) = tier_name
            && let Some(cfg) = project_config
        {
            let tier_preference_order = vec![tool.as_str().to_string()];
            let resolution = if force_override_user_config {
                force_resolve_review_tool_from_tier(tier, cfg, global_config, model_catalog, tool)
                    .map_or_else(
                    || {
                        crate::run_helpers::resolve_preferred_tool_from_tier_with_catalog(
                            tier,
                            cfg,
                            Some(global_config),
                            model_catalog,
                            parent_tool,
                            &tier_preference_order,
                            &[],
                        )
                    },
                    Ok,
                )?
            } else {
                crate::run_helpers::resolve_preferred_tool_from_tier_with_catalog(
                    tier,
                    cfg,
                    Some(global_config),
                    model_catalog,
                    parent_tool,
                    &tier_preference_order,
                    &[],
                )?
            };
            return Ok(ResolvedReviewSelection {
                tool: resolution.tool,
                model_spec: Some(resolution.model_spec),
                tier_preference_order,
            });
        }

        if let Some(cfg) = project_config {
            cfg.enforce_tool_enabled(tool.as_str(), force_override_user_config)?;
        }
        return Ok(ResolvedReviewSelection {
            tool,
            model_spec: None,
            tier_preference_order: Vec::new(),
        });
    }

    let effective_selection = project_config
        .and_then(|cfg| cfg.review.as_ref())
        .map(|r| &r.tool)
        .unwrap_or(&global_config.review.tool);
    let tier_preference_order = effective_selection.preference_order();

    if let Some(ref tier) = tier_name {
        let cfg = project_config.ok_or_else(|| {
            anyhow::anyhow!(
                "Review tier '{}' is configured, but no tier definitions are available. \
                 Run `csa init --full` or define [tiers.*] in config.",
                tier
            )
        })?;

        let tier_tools = cfg.list_tools_in_tier(tier);

        if let Some(resolution) = crate::run_helpers::resolve_tool_from_tier_with_catalog(
            tier,
            cfg,
            Some(global_config),
            model_catalog,
            parent_tool,
            &tier_preference_order,
            &[],
        )? {
            return Ok(ResolvedReviewSelection {
                tool: resolution.tool,
                model_spec: Some(resolution.model_spec),
                tier_preference_order,
            });
        }

        let available_tools_after_checks =
            crate::run_helpers::collect_available_tier_models_with_catalog(
                tier,
                cfg,
                Some(global_config),
                model_catalog,
                &[],
            )?;
        let (_, excluded_models_after_checks) =
            crate::run_helpers::evaluate_tier_models_with_catalog(
                tier,
                cfg,
                Some(global_config),
                model_catalog,
                &[],
            )?;
        let configured_tools: Vec<&str> = tier_tools
            .iter()
            .map(|(tool_name, _)| tool_name.as_str())
            .collect();
        let available_tools: Vec<&str> = available_tools_after_checks
            .iter()
            .map(|resolution| resolution.tool.as_str())
            .collect();
        let excluded_models: Vec<String> = excluded_models_after_checks
            .iter()
            .map(|exclusion| format!("{}={}", exclusion.model_spec, exclusion.kind.category()))
            .collect();
        anyhow::bail!(
            "Tier '{}' resolved for review, but none of its tools are currently available.\n\
             Configured tier tools: [{}].\n\
             Available tier tools after enablement/install checks: [{}].\n\
             Excluded tier models after routing checks: [{}].",
            tier,
            configured_tools.join(", "),
            available_tools.join(", "),
            excluded_models.join(", ")
        );
    }

    if let Some(project_review) = project_config.and_then(|cfg| cfg.review.as_ref()) {
        return super::resolve_review_tool_from_selection(
            &project_review.tool,
            parent_tool,
            project_config,
            global_config,
            project_root,
        )
        .map(|t| ResolvedReviewSelection {
            tool: t,
            model_spec: None,
            tier_preference_order: Vec::new(),
        })
        .with_context(|| {
            format!(
                "Failed to resolve review tool from project config: {}",
                ProjectConfig::config_path(project_root).display()
            )
        });
    }

    // Global config tool selection
    super::resolve_review_tool_from_selection(
        &global_config.review.tool,
        parent_tool,
        project_config,
        global_config,
        project_root,
    )
    .map(|t| ResolvedReviewSelection {
        tool: t,
        model_spec: None,
        tier_preference_order: Vec::new(),
    })
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_review_selection(
    arg_tool: Option<ToolName>,
    arg_model_spec: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    parent_tool: Option<&str>,
    project_root: &Path,
    force_override_user_config: bool,
    cli_tier: Option<&str>,
    force_ignore_tier_setting: bool,
    direct_tool_requested: bool,
) -> Result<ResolvedReviewSelection> {
    let catalog = csa_config::EffectiveModelCatalog::shipped()?;
    resolve_review_selection_with_catalog(
        arg_tool,
        arg_model_spec,
        project_config,
        global_config,
        &catalog,
        parent_tool,
        project_root,
        force_override_user_config,
        cli_tier,
        force_ignore_tier_setting,
        direct_tool_requested,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_review_tool(
    arg_tool: Option<ToolName>,
    arg_model_spec: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    parent_tool: Option<&str>,
    project_root: &Path,
    force_override_user_config: bool,
    cli_tier: Option<&str>,
    force_ignore_tier_setting: bool,
) -> Result<(ToolName, Option<String>)> {
    let resolved = resolve_review_selection(
        arg_tool,
        arg_model_spec,
        project_config,
        global_config,
        parent_tool,
        project_root,
        force_override_user_config,
        cli_tier,
        force_ignore_tier_setting,
        arg_tool.is_some(),
    )?;
    Ok((resolved.tool, resolved.model_spec))
}
