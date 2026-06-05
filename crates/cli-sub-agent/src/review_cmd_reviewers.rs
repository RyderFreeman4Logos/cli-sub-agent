use anyhow::Result;
use tracing::{info, warn};

use crate::cli::ReviewArgs;
use crate::review_consensus::{build_reviewer_tools, validate_multi_reviewer_tier_pool};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;

const MAX_AUTO_HETEROGENEOUS_REVIEWERS: usize = 3;

pub(crate) struct AutoReviewerSelection {
    pub(crate) reviewers: usize,
    pub(crate) selected_tools: Vec<ToolName>,
}

pub(crate) struct EffectiveReviewerSelection {
    pub(crate) reviewers: usize,
    pub(crate) selected_tools: Option<Vec<ToolName>>,
}

pub(crate) struct AutoReviewerRequest<'a> {
    pub(crate) requested_reviewers: usize,
    pub(crate) explicit_reviewer_count: bool,
    pub(crate) single: bool,
    pub(crate) scope_is_range: bool,
    pub(crate) large_diff_auto_escalation: bool,
    pub(crate) explicit_tool: Option<ToolName>,
    pub(crate) explicit_model_spec: Option<&'a str>,
    pub(crate) primary_tool: ToolName,
    pub(crate) resolved_tier_name: Option<&'a str>,
    pub(crate) config: Option<&'a ProjectConfig>,
    pub(crate) global_config: &'a GlobalConfig,
}

pub(crate) struct MultiReviewerPool {
    pub(crate) reviewer_tools: Vec<ToolName>,
    pub(crate) tier_reviewer_specs: Vec<crate::run_helpers::TierToolResolution>,
}

fn collect_tier_reviewer_specs(
    resolved_tier_name: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Vec<crate::run_helpers::TierToolResolution> {
    resolved_tier_name
        .and_then(|tier_name| {
            config.map(|cfg| {
                let effective_selection = cfg
                    .review
                    .as_ref()
                    .map(|review| &review.tool)
                    .unwrap_or(&global_config.review.tool);
                crate::run_helpers::collect_preferred_tier_models(
                    tier_name,
                    cfg,
                    &effective_selection.preference_order(),
                    &[],
                )
            })
        })
        .unwrap_or_default()
}

fn effective_review_tool_preferences(
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Vec<String> {
    config
        .and_then(|cfg| cfg.review.as_ref())
        .map(|review| &review.tool)
        .unwrap_or(&global_config.review.tool)
        .preference_order()
}

fn collect_unique_tier_tools(
    tier_reviewer_specs: &[crate::run_helpers::TierToolResolution],
) -> Vec<ToolName> {
    let mut tier_reviewer_tools = Vec::new();
    for resolution in tier_reviewer_specs {
        if !tier_reviewer_tools.contains(&resolution.tool) {
            tier_reviewer_tools.push(resolution.tool);
        }
    }
    tier_reviewer_tools
}

fn build_selected_tool_subset(
    primary_tool: ToolName,
    tier_reviewer_tools: &[ToolName],
    reviewers: usize,
    _preference_order: &[String],
) -> Vec<ToolName> {
    let mut selected = Vec::new();
    selected.push(primary_tool);
    for tool in tier_reviewer_tools {
        if !selected.contains(tool) {
            selected.push(*tool);
        }
    }
    selected.truncate(reviewers);
    selected
}

fn build_auto_heterogeneous_tool_subset(
    primary_tool: ToolName,
    tier_reviewer_tools: &[ToolName],
    reviewers: usize,
) -> Vec<ToolName> {
    let mut selected = vec![primary_tool];
    let mut selected_families = vec![primary_tool.model_family()];
    let mut same_family_candidates = Vec::new();

    for tool in tier_reviewer_tools {
        if selected.contains(tool) {
            continue;
        }

        let family = tool.model_family();
        if selected_families.contains(&family) {
            same_family_candidates.push(*tool);
        } else {
            selected_families.push(family);
            selected.push(*tool);
            if selected.len() == reviewers {
                return selected;
            }
        }
    }

    for tool in same_family_candidates {
        if selected.len() == reviewers {
            break;
        }
        selected.push(tool);
    }

    selected
}

fn has_at_least_two_model_families(tools: &[ToolName]) -> bool {
    let mut families = Vec::new();
    for tool in tools {
        let family = tool.model_family();
        if !families.contains(&family) {
            families.push(family);
            if families.len() >= 2 {
                return true;
            }
        }
    }
    false
}

fn repeat_reviewer_pool(pool: &[ToolName], reviewer_count: usize) -> Vec<ToolName> {
    (0..reviewer_count)
        .map(|idx| pool[idx % pool.len()])
        .collect()
}

pub(crate) fn resolve_auto_reviewer_selection(
    request: &AutoReviewerRequest<'_>,
) -> Option<AutoReviewerSelection> {
    if request.requested_reviewers != 1
        || request.explicit_reviewer_count
        || request.single
        || !(request.scope_is_range || request.large_diff_auto_escalation)
        || request.explicit_tool.is_some()
        || request.explicit_model_spec.is_some()
    {
        return None;
    }

    let tier_reviewer_specs = collect_tier_reviewer_specs(
        request.resolved_tier_name,
        request.config,
        request.global_config,
    );
    let tier_reviewer_tools = collect_unique_tier_tools(&tier_reviewer_specs);
    let unique_pool = build_auto_heterogeneous_tool_subset(
        request.primary_tool,
        &tier_reviewer_tools,
        MAX_AUTO_HETEROGENEOUS_REVIEWERS,
    );

    (unique_pool.len() >= 2 && has_at_least_two_model_families(&unique_pool)).then_some(
        AutoReviewerSelection {
            reviewers: unique_pool.len(),
            selected_tools: unique_pool,
        },
    )
}

pub(crate) fn resolve_effective_reviewer_selection(
    request: &AutoReviewerRequest<'_>,
) -> EffectiveReviewerSelection {
    let auto_reviewer_selection = resolve_auto_reviewer_selection(request);
    if let Some(selection) = auto_reviewer_selection {
        let tool_list = selection
            .selected_tools
            .iter()
            .map(|tool| tool.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        info!(
            "Auto-selected {} heterogeneous reviewers from tier '{}': {}",
            selection.reviewers,
            request
                .resolved_tier_name
                .unwrap_or("no tier name resolved"),
            tool_list
        );
        EffectiveReviewerSelection {
            reviewers: selection.reviewers,
            selected_tools: Some(selection.selected_tools),
        }
    } else {
        EffectiveReviewerSelection {
            reviewers: request.requested_reviewers,
            selected_tools: None,
        }
    }
}

pub(crate) fn resolve_effective_reviewer_selection_for_args(
    args: &ReviewArgs,
    large_diff_auto_escalation: bool,
    primary_tool: ToolName,
    resolved_tier_name: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> EffectiveReviewerSelection {
    resolve_effective_reviewer_selection(&AutoReviewerRequest {
        requested_reviewers: args.requested_reviewers() as usize,
        explicit_reviewer_count: args.reviewers.is_some(),
        single: args.single,
        scope_is_range: args.range.is_some(),
        large_diff_auto_escalation,
        explicit_tool: super::prior_rounds::explicit_review_tool(args),
        explicit_model_spec: args.model_spec.as_deref(),
        primary_tool,
        resolved_tier_name,
        config,
        global_config,
    })
}

pub(crate) fn resolve_multi_reviewer_pool(
    reviewers: usize,
    selected_reviewer_tools: Option<&[ToolName]>,
    explicit_tool: Option<ToolName>,
    primary_tool: ToolName,
    resolved_tier_name: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Result<MultiReviewerPool> {
    let tier_reviewer_specs =
        collect_tier_reviewer_specs(resolved_tier_name, config, global_config);
    let tier_reviewer_tools = collect_unique_tier_tools(&tier_reviewer_specs);
    let preference_order = effective_review_tool_preferences(config, global_config);

    if let Some(tier_name) = resolved_tier_name {
        let unique_reviewer_tools = validate_multi_reviewer_tier_pool(
            tier_name,
            reviewers,
            primary_tool,
            &tier_reviewer_tools,
        )?;
        if reviewers > unique_reviewer_tools {
            warn!(
                tier = tier_name,
                requested_reviewers = reviewers,
                unique_tools = unique_reviewer_tools,
                "Multi-reviewer tier pool will reuse tools because fewer unique tier reviewers are available than requested"
            );
        }
    }

    let reviewer_tools = if let Some(selected_tools) = selected_reviewer_tools {
        selected_tools.to_vec()
    } else if resolved_tier_name.is_some() && explicit_tool.is_none() {
        let pool = build_selected_tool_subset(
            primary_tool,
            &tier_reviewer_tools,
            usize::MAX,
            &preference_order,
        );
        if pool.is_empty() {
            anyhow::bail!("Review tier resolved no reviewer tools");
        }
        repeat_reviewer_pool(&pool, reviewers)
    } else {
        build_reviewer_tools(
            explicit_tool,
            primary_tool,
            config,
            Some(global_config),
            resolved_tier_name.map(|_| tier_reviewer_tools.as_slice()),
            reviewers,
        )
    };

    Ok(MultiReviewerPool {
        reviewer_tools,
        tier_reviewer_specs,
    })
}

#[cfg(test)]
#[path = "review_cmd_reviewers_tests.rs"]
mod tests;
