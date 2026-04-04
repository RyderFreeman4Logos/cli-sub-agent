use anyhow::Result;
use tracing::warn;

use crate::review_consensus::{build_reviewer_tools, validate_multi_reviewer_tier_pool};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;

pub(crate) struct MultiReviewerPool {
    pub(crate) reviewer_tools: Vec<ToolName>,
    pub(crate) tier_reviewer_specs: Vec<crate::run_helpers::TierToolResolution>,
}

pub(crate) fn resolve_multi_reviewer_pool(
    reviewers: usize,
    explicit_tool: Option<ToolName>,
    primary_tool: ToolName,
    resolved_tier_name: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Result<MultiReviewerPool> {
    let tier_reviewer_specs = resolved_tier_name
        .and_then(|tier_name| {
            config.map(|cfg| {
                let effective_selection = cfg
                    .review
                    .as_ref()
                    .map(|review| &review.tool)
                    .unwrap_or(&global_config.review.tool);
                crate::run_helpers::collect_available_tier_models(
                    tier_name,
                    cfg,
                    effective_selection.whitelist(),
                    &[],
                )
            })
        })
        .unwrap_or_default();

    let mut tier_reviewer_tools = Vec::new();
    for resolution in &tier_reviewer_specs {
        if !tier_reviewer_tools.contains(&resolution.tool) {
            tier_reviewer_tools.push(resolution.tool);
        }
    }

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

    let reviewer_tools = build_reviewer_tools(
        explicit_tool,
        primary_tool,
        config,
        Some(global_config),
        resolved_tier_name.map(|_| tier_reviewer_tools.as_slice()),
        reviewers,
    );

    Ok(MultiReviewerPool {
        reviewer_tools,
        tier_reviewer_specs,
    })
}
