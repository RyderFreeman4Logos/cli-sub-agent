use anyhow::Result;
use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_executor::{Executor, ModelSpec, ThinkingBudget};

use super::tool_availability;

/// Build an executor from tool, model_spec, model, and thinking parameters.
pub(crate) fn build_executor(
    tool: &ToolName,
    model_spec: Option<&str>,
    model: Option<&str>,
    thinking: Option<&str>,
    config: Option<&ProjectConfig>,
    apply_tool_defaults: bool,
) -> Result<Executor> {
    let mut executor = if let Some(spec) = model_spec {
        let parsed = ModelSpec::parse(spec)?;
        Executor::from_spec(&parsed)?
    } else {
        let tool_name = tool.as_str();
        let (parsed_model, model_thinking) = match model {
            Some(m) => {
                let (clean, budget) = ThinkingBudget::try_split_from_model(m);
                (Some(clean.to_string()), budget)
            }
            None => (None, None),
        };
        let final_model = parsed_model.or_else(|| {
            apply_tool_defaults.then(|| {
                config.and_then(|cfg| {
                    cfg.tool_default_model(tool_name)
                        .map(|default_model| cfg.resolve_alias(default_model))
                })
            })?
        });
        let effective_thinking = thinking.or_else(|| {
            apply_tool_defaults
                .then(|| config.and_then(|cfg| cfg.tool_default_thinking(tool_name)))?
        });
        let budget = effective_thinking
            .map(ThinkingBudget::parse)
            .transpose()?
            .or(model_thinking);
        Executor::from_tool_name(tool, final_model, budget)
    };

    if model_spec.is_some() {
        if let Some(explicit_model) = model {
            let (clean, suffix_budget) = ThinkingBudget::try_split_from_model(explicit_model);
            executor.override_model(clean.to_string());
            if thinking.is_none()
                && let Some(budget) = suffix_budget
            {
                executor.override_thinking_budget(budget);
            }
        }
        if let Some(explicit_thinking) = thinking {
            executor.override_thinking_budget(ThinkingBudget::parse(explicit_thinking)?);
        }
    }

    if matches!(executor, Executor::Codex { .. }) {
        executor.override_codex_transport(tool_availability::resolved_codex_transport(config));
    }
    if matches!(executor, Executor::ClaudeCode { .. }) {
        executor.override_claude_code_transport(tool_availability::resolved_claude_code_transport(
            config,
        ));
    }
    Ok(executor)
}

pub(crate) fn model_name_for_tier_validation(model: Option<&str>) -> Option<&str> {
    model.map(|name| ThinkingBudget::try_split_from_model(name).0)
}
