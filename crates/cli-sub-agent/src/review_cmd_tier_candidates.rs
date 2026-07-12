use anyhow::Result;
use csa_config::{EffectiveModelCatalog, GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;

pub(super) struct ReviewTierCandidateRequest<'a> {
    pub(super) initial_tool: ToolName,
    pub(super) initial_model_spec: Option<&'a str>,
    pub(super) tier_name: Option<&'a str>,
    pub(super) project_config: Option<&'a ProjectConfig>,
    pub(super) global_config: Option<&'a GlobalConfig>,
    pub(super) model_catalog: &'a EffectiveModelCatalog,
    pub(super) tier_fallback_enabled: bool,
    pub(super) no_failover: bool,
    pub(super) tier_preference_order: &'a [String],
}

pub(super) fn review_ordered_tier_candidates(
    request: ReviewTierCandidateRequest<'_>,
) -> Result<Vec<(ToolName, Option<String>)>> {
    crate::tier_model_fallback::ordered_tier_candidates_with_catalog(
        request.initial_tool,
        request.initial_model_spec,
        request.tier_name,
        request.project_config,
        request.global_config,
        request.model_catalog,
        crate::tier_model_fallback::TierFallbackOptions {
            enabled: request.tier_fallback_enabled && !request.no_failover,
            preference_order: request.tier_preference_order,
        },
    )
}
