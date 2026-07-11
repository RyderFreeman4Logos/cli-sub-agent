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
        if parsed.tool != tool.as_str() {
            anyhow::bail!(
                "tool/model-spec mismatch: selected tool {} cannot execute model spec {spec} \
                 because it selects tool {}; refusing to dispatch through the wrong provider",
                tool.as_str(),
                parsed.tool
            );
        }
        Executor::from_spec(&parsed)?
    } else {
        let tool_name = tool.as_str();
        let selected_model = model
            .map(|value| {
                config
                    .map(|cfg| cfg.resolve_alias(value))
                    .unwrap_or_else(|| value.to_string())
            })
            .or_else(|| {
                apply_tool_defaults.then(|| {
                    config.and_then(|cfg| {
                        cfg.tool_default_model(tool_name)
                            .map(|default_model| cfg.resolve_alias(default_model))
                    })
                })?
            });
        if let Some(full_spec) = selected_model
            .as_deref()
            .filter(|value| value.matches('/').count() == 3)
        {
            let parsed = ModelSpec::parse(full_spec)?;
            if parsed.tool != tool_name {
                anyhow::bail!(
                    "tool/model-spec mismatch: selected tool {} cannot execute model spec {full_spec} \
                     because it selects tool {}; refusing to dispatch through the wrong provider",
                    tool_name,
                    parsed.tool
                );
            }
            let mut executor = Executor::from_spec(&parsed)?;
            let effective_thinking = thinking.or_else(|| {
                apply_tool_defaults
                    .then(|| config.and_then(|cfg| cfg.tool_default_thinking(tool_name)))?
            });
            if let Some(value) = effective_thinking {
                executor.override_thinking_budget(ThinkingBudget::parse(value)?);
            }
            executor
        } else {
            let (parsed_model, model_thinking) = match selected_model.as_deref() {
                Some(model) => {
                    let (clean, budget) = ThinkingBudget::try_split_from_model(model);
                    (Some(clean.to_string()), budget)
                }
                None => (None, None),
            };
            let effective_thinking = thinking.or_else(|| {
                apply_tool_defaults
                    .then(|| config.and_then(|cfg| cfg.tool_default_thinking(tool_name)))?
            });
            let budget = effective_thinking
                .map(ThinkingBudget::parse)
                .transpose()?
                .or(model_thinking);
            Executor::from_tool_name(tool, parsed_model, budget)
        }
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
        executor.set_codex_tmux_mode(config.is_some_and(ProjectConfig::codex_tmux_mode));
    }
    if matches!(executor, Executor::ClaudeCode { .. }) {
        executor.override_claude_code_transport(tool_availability::resolved_claude_code_transport(
            config,
        ));
    }
    Ok(executor)
}

pub(crate) fn model_name_for_tier_validation(model: Option<&str>) -> Option<&str> {
    model.map(|name| {
        if name.split('/').count() == 4 {
            name
        } else {
            ThinkingBudget::try_split_from_model(name).0
        }
    })
}
