use std::collections::HashMap;

use anyhow::{Result, bail};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_executor::ModelSpec;
use weave::compiler::PlanStep;
use weave::parser::WorkspaceAccess;

use super::substitute_vars;

/// Resolved execution target for a plan step.
/// Keeps direct shell execution separate from AI dispatch so `tool = "bash"` never falls through.
pub(crate) enum StepTarget {
    /// Execute bash code block directly via `tokio::process::Command`.
    DirectBash,
    /// Skip this step (compile-time INCLUDE directive from weave).
    WeaveInclude,
    /// Non-executable note for human-facing workflow context.
    Note,
    /// Manual action that must be handled by the orchestrator, not CSA.
    Manual,
    /// Stop the workflow and wait for explicit user action before any rerun.
    AwaitUser,
    /// Dispatch to an AI tool via CSA infrastructure.
    CsaTool {
        tool_name: ToolName,
        model_spec: Option<String>,
        tier_name: Option<String>,
    },
}

impl StepTarget {
    fn csa(tool: ToolName, spec: Option<String>) -> Self {
        Self::CsaTool {
            tool_name: tool,
            model_spec: spec,
            tier_name: None,
        }
    }

    fn csa_with_tier(tool: ToolName, spec: Option<String>, tier: String) -> Self {
        Self::CsaTool {
            tool_name: tool,
            model_spec: spec,
            tier_name: Some(tier),
        }
    }
}

/// Resolve a step target from its annotations and config.
/// Order: CLI overrides, explicit `step.tool`, then `step.tier`, then configured default or codex fallback.
pub(crate) fn resolve_step_tool(
    step: &PlanStep,
    config: Option<&ProjectConfig>,
    tool_override: Option<&ToolName>,
    model_spec_override: Option<&String>,
) -> Result<StepTarget> {
    // CLI overrides only apply to CSA-dispatched steps. Deterministic
    // workflow directives must remain deterministic even when --tool is set.
    let explicit_tool = step.tool.as_deref().map(str::to_ascii_lowercase);
    if let Some(tool) = tool_override
        && !is_deterministic_step_tool(explicit_tool.as_deref())
    {
        return Ok(StepTarget::csa(*tool, model_spec_override.cloned()));
    }
    if let Some(model_spec) = model_spec_override
        && !is_deterministic_step_tool(explicit_tool.as_deref())
    {
        let spec = ModelSpec::parse(model_spec)?;
        let tool = parse_tool_name(&spec.tool)?;
        return Ok(StepTarget::csa(tool, Some(model_spec.clone())));
    }

    if let Some(tool_lower) = explicit_tool {
        match tool_lower.as_str() {
            "bash" => return Ok(StepTarget::DirectBash),
            "note" => return Ok(StepTarget::Note),
            "manual" => return Ok(StepTarget::Manual),
            "await-user" => return Ok(StepTarget::AwaitUser),
            "gemini-cli" => return Ok(StepTarget::csa(ToolName::GeminiCli, None)),
            "antigravity-cli" => return Ok(StepTarget::csa(ToolName::AntigravityCli, None)),
            "opencode" => return Ok(StepTarget::csa(ToolName::Opencode, None)),
            "codex" => return Ok(StepTarget::csa(ToolName::Codex, None)),
            "claude-code" => return Ok(StepTarget::csa(ToolName::ClaudeCode, None)),
            "csa" => {
                if let Some(target) = resolve_csa_step_target(step, config)? {
                    return Ok(target);
                }
                return Ok(StepTarget::csa(ToolName::Codex, None));
            }
            "weave" => return Ok(StepTarget::WeaveInclude),
            other => bail!(
                "Unknown tool '{}' in step {} ('{}'). Known: bash, note, manual, await-user, gemini-cli, opencode, codex, claude-code, csa, weave",
                other,
                step.id,
                step.title
            ),
        }
    }

    if let Some(ref tier_name) = step.tier {
        if let Some(cfg) = config {
            if let Some(target) = resolve_configured_tier_target(cfg, tier_name)? {
                return Ok(target);
            }
            tracing::warn!(
                "Tier '{}' not found or no enabled tools; falling back to codex for step {}",
                tier_name,
                step.id
            );
        }
        return Ok(StepTarget::csa(ToolName::Codex, None));
    }

    if let Some(cfg) = config
        && let Some(target) = resolve_default_tier_target(cfg)?
    {
        return Ok(target);
    }

    Ok(StepTarget::csa(ToolName::Codex, None))
}

/// Resolve a step target after applying workflow variables to the step tier.
pub(crate) fn resolve_step_tool_with_variables(
    step: &PlanStep,
    variables: &HashMap<String, String>,
    config: Option<&ProjectConfig>,
    tool_override: Option<&ToolName>,
    model_spec_override: Option<&String>,
) -> Result<StepTarget> {
    let Some(tier) = step.tier.as_deref() else {
        return resolve_step_tool(step, config, tool_override, model_spec_override);
    };
    let resolved_tier = substitute_vars(tier, variables);
    if resolved_tier == tier {
        return resolve_step_tool(step, config, tool_override, model_spec_override);
    }
    let mut resolved_step = step.clone();
    resolved_step.tier = Some(resolved_tier);
    resolve_step_tool(&resolved_step, config, tool_override, model_spec_override)
}

pub(crate) fn step_readonly_project_root(step: &PlanStep) -> bool {
    matches!(step.workspace_access, Some(WorkspaceAccess::ReadOnly))
}

fn resolve_csa_step_target(
    step: &PlanStep,
    config: Option<&ProjectConfig>,
) -> Result<Option<StepTarget>> {
    let Some(cfg) = config else {
        return Ok(None);
    };
    if let Some(ref tier_name) = step.tier
        && let Some(target) = resolve_configured_tier_target(cfg, tier_name)?
    {
        return Ok(Some(target));
    }
    resolve_default_tier_target(cfg)
}

fn resolve_configured_tier_target(
    config: &ProjectConfig,
    tier_name: &str,
) -> Result<Option<StepTarget>> {
    let Some(tier) = config.tiers.get(tier_name) else {
        return Ok(None);
    };
    for model_spec_str in &tier.models {
        let parts: Vec<&str> = model_spec_str.splitn(4, '/').collect();
        if parts.len() == 4 && config.is_tool_enabled(parts[0]) {
            let tool = parse_tool_name(parts[0])?;
            return Ok(Some(StepTarget::csa_with_tier(
                tool,
                Some(model_spec_str.clone()),
                tier_name.to_string(),
            )));
        }
    }
    Ok(None)
}

fn resolve_default_tier_target(config: &ProjectConfig) -> Result<Option<StepTarget>> {
    let Some((_tool_name, model_spec)) = config.resolve_tier_tool("default") else {
        return Ok(None);
    };
    let spec = ModelSpec::parse(&model_spec)?;
    let tool = parse_tool_name(&spec.tool)?;
    Ok(Some(StepTarget::csa(tool, Some(model_spec))))
}

fn is_deterministic_step_tool(explicit_tool: Option<&str>) -> bool {
    matches!(
        explicit_tool,
        Some("bash" | "note" | "manual" | "await-user" | "weave")
    )
}

fn parse_tool_name(tool: &str) -> Result<ToolName> {
    match tool {
        "gemini-cli" => Ok(ToolName::GeminiCli),
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        "claude-code" => Ok(ToolName::ClaudeCode),
        "antigravity-cli" => Ok(ToolName::AntigravityCli),
        other => bail!("Unknown tool: {other}"),
    }
}
