use anyhow::Result;
use csa_config::{ExecutionEnvOptions, GlobalConfig, ProjectConfig};
use csa_core::types::ModelFamily;
use tracing::warn;

use super::RateLimitAction;

pub(super) struct FailoverAvailabilityRequest<'a> {
    pub failed_tool: &'a str,
    pub task_type: &'a str,
    pub resolved_tier_name: Option<&'a str>,
    pub required_tool: Option<&'a str>,
    pub task_needs_edit: Option<bool>,
    pub session_state: Option<&'a csa_session::MetaSessionState>,
    pub exhausted_providers: &'a [ModelFamily],
    pub config: &'a ProjectConfig,
    pub global_config: Option<&'a GlobalConfig>,
    pub model_catalog: &'a csa_config::EffectiveModelCatalog,
    pub original_error: &'a str,
}

pub(super) struct FailoverAvailabilityState<'a> {
    pub tried_tools: &'a mut Vec<String>,
    pub tried_specs: &'a mut Vec<String>,
}

pub(super) fn decide_available_failover(
    request: FailoverAvailabilityRequest<'_>,
    state: FailoverAvailabilityState<'_>,
) -> Result<RateLimitAction> {
    let FailoverAvailabilityRequest {
        failed_tool,
        task_type,
        resolved_tier_name,
        required_tool,
        task_needs_edit,
        session_state,
        exhausted_providers,
        config,
        global_config,
        model_catalog,
        original_error,
    } = request;
    let FailoverAvailabilityState {
        tried_tools,
        tried_specs,
    } = state;

    let max_candidates = config
        .tiers
        .values()
        .map(|tier| tier.models.len())
        .sum::<usize>()
        .saturating_add(csa_config::global::all_known_tools().len())
        .max(1);

    for _ in 0..max_candidates {
        let action = csa_scheduler::decide_failover(
            failed_tool,
            task_type,
            resolved_tier_name,
            task_needs_edit,
            session_state,
            tried_tools,
            tried_specs,
            exhausted_providers,
            config,
            original_error,
        );

        let (new_tool, new_model_spec) = match action {
            csa_scheduler::FailoverAction::RetryInSession {
                new_tool,
                new_model_spec,
                session_id: _,
            }
            | csa_scheduler::FailoverAction::RetrySiblingSession {
                new_tool,
                new_model_spec,
            } => (new_tool, new_model_spec),
            csa_scheduler::FailoverAction::ReportError { reason, .. } => {
                return Ok(RateLimitAction::ExhaustedFailovers {
                    reason: match required_tool {
                        Some(tool) => explicit_tool_exhausted_reason(tool),
                        None => reason,
                    },
                });
            }
        };

        if let Some(required_tool) = required_tool
            && new_tool != required_tool
        {
            warn!(
                required_tool = %required_tool,
                skipped_tool = %new_tool,
                model_spec = %new_model_spec,
                "[csa-failover] skipping cross-tool fallback candidate for explicit --tool tier run"
            );
            if !tried_tools.iter().any(|tool| tool == &new_tool) {
                tried_tools.push(new_tool);
            }
            if !tried_specs.iter().any(|spec| spec == &new_model_spec) {
                tried_specs.push(new_model_spec);
            }
            continue;
        }

        if let Err(reason) = crate::run_helpers::validate_tier_model_spec_compatibility_with_catalog(
            &new_model_spec,
            model_catalog,
        ) {
            warn!(
                tool = %new_tool,
                model_spec = %new_model_spec,
                reason = ?reason,
                "[csa-failover] skipping catalog-rejected fallback candidate"
            );
            if !tried_specs.iter().any(|spec| spec == &new_model_spec) {
                tried_specs.push(new_model_spec);
            }
            continue;
        }

        let extra_env = global_config
            .and_then(|cfg| cfg.build_execution_env(&new_tool, ExecutionEnvOptions::default()));
        match crate::run_helpers::tool_runtime_availability_with_env(
            &new_tool,
            Some(config),
            Some(&new_model_spec),
            extra_env.as_ref(),
        ) {
            crate::run_helpers::ToolBinaryAvailability::Available { .. } => {
                let tool = crate::run_helpers::parse_tool_name(&new_tool)?;
                return Ok(RateLimitAction::Retry {
                    new_tool: tool,
                    new_model_spec: Some(new_model_spec),
                });
            }
            crate::run_helpers::ToolBinaryAvailability::Missing { hint, .. } => {
                warn!(
                    tool = %new_tool,
                    model_spec = %new_model_spec,
                    hint = %hint,
                    "[csa-failover] skipping unavailable fallback candidate"
                );
                if !tried_tools.iter().any(|tool| tool == &new_tool) {
                    tried_tools.push(new_tool);
                }
                if !tried_specs.iter().any(|spec| spec == &new_model_spec) {
                    tried_specs.push(new_model_spec);
                }
            }
        }
    }

    Ok(RateLimitAction::ExhaustedFailovers {
        reason: match required_tool {
            Some(tool) => explicit_tool_exhausted_reason(tool),
            None => "no executable tier fallback candidates remain".to_string(),
        },
    })
}

fn explicit_tool_exhausted_reason(tool: &str) -> String {
    format!(
        "no executable {tool} fallback candidates remain; explicit --tool {tool} prevents cross-tool tier failover"
    )
}

#[cfg(test)]
#[path = "run_cmd_post_failover_catalog_tests.rs"]
mod catalog_tests;
