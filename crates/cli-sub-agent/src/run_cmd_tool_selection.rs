//! Tool selection, session selection, and failover helpers for `csa run`.
//!
//! Extracted from `run_cmd.rs` to keep module sizes manageable.
use std::path::Path;

use anyhow::Result;
use tracing::warn;

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{ToolName, ToolSelectionStrategy};
use csa_session::{MetaSessionState, SessionPhase, resolve_session_prefix};
use weave::parser::AgentConfig;

use crate::cli::ReturnTarget;
use crate::run_helpers::{
    detect_parent_tool, parse_tool_name, read_prompt, resolve_tool, resolve_tool_and_model,
};
use crate::skill_resolver::{self, ResolvedSkill};

/// Resolve the `--last` flag to a concrete session ID.
///
/// Returns the most recently accessed session ID plus an optional warning
/// string when the selection is ambiguous (multiple active sessions).
pub(crate) fn resolve_last_session_selection(
    sessions: Vec<MetaSessionState>,
) -> Result<(String, Option<String>)> {
    if sessions.is_empty() {
        anyhow::bail!("No sessions found. Run a task first to create one.");
    }

    let mut sorted_sessions = sessions;
    sorted_sessions.sort_by_key(|session| std::cmp::Reverse(session.last_accessed));
    let selected_id = sorted_sessions[0].meta_session_id.clone();

    let active_sessions: Vec<&MetaSessionState> = sorted_sessions
        .iter()
        .filter(|session| session.phase == SessionPhase::Active)
        .collect();

    if active_sessions.len() <= 1 {
        return Ok((selected_id, None));
    }

    let mut warning_lines = vec![
        format!(
            "warning: `--last` is ambiguous in this project: found {} active sessions.",
            active_sessions.len()
        ),
        format!("Resuming most recently accessed session: {}", selected_id),
        "Active sessions (session_id | last_accessed):".to_string(),
    ];

    for session in active_sessions {
        warning_lines.push(format!(
            "  {} | {}",
            session.meta_session_id,
            session.last_accessed.to_rfc3339()
        ));
    }

    warning_lines.push("Use `--session <session-id>` to choose explicitly.".to_string());

    Ok((selected_id, Some(warning_lines.join("\n"))))
}

/// Filter enabled tools to those from a different model family than the parent.
pub(crate) fn resolve_heterogeneous_candidates(
    parent_tool: &ToolName,
    enabled_tools: &[ToolName],
) -> Vec<ToolName> {
    let parent_family = parent_tool.model_family();
    enabled_tools
        .iter()
        .copied()
        .filter(|tool| tool.model_family() != parent_family)
        .collect()
}

/// Pop the next untried heterogeneous tool from the candidate list.
pub(crate) fn take_next_runtime_fallback_tool(
    candidates: &mut Vec<ToolName>,
    current_tool: ToolName,
    tried_tools: &[String],
) -> Option<ToolName> {
    while let Some(candidate) = candidates.first().copied() {
        candidates.remove(0);
        if candidate == current_tool {
            continue;
        }
        if tried_tools.iter().any(|tried| tried == candidate.as_str()) {
            continue;
        }
        return Some(candidate);
    }
    None
}

/// Read the slot wait timeout from project config or fall back to the default.
pub(crate) fn resolve_slot_wait_timeout_seconds(config: Option<&ProjectConfig>) -> u64 {
    config
        .map(|cfg| cfg.resources.slot_wait_timeout_seconds)
        .unwrap_or(csa_config::ResourcesConfig::default().slot_wait_timeout_seconds)
}

/// Resolve a session prefix (short ID) to a full session ID.
pub(crate) fn resolve_session_reference(project_root: &Path, session_ref: &str) -> Result<String> {
    let sessions_dir = csa_session::get_session_root(project_root)?.join("sessions");
    resolve_session_prefix(&sessions_dir, session_ref)
}

/// Result of strategy-based tool resolution.
pub(crate) struct StrategyResolution {
    pub(crate) tool: ToolName,
    pub(crate) model_spec: Option<String>,
    pub(crate) model: Option<String>,
    /// Canonical tier name used for the initial selection, when routing resolved via a tier.
    pub(crate) resolved_tier_name: Option<String>,
    /// Remaining heterogeneous candidates for runtime fallback (HeterogeneousPreferred only).
    pub(crate) runtime_fallback_candidates: Vec<ToolName>,
}

fn resolve_default_tier_name(config: Option<&ProjectConfig>) -> Option<String> {
    let cfg = config?;
    cfg.tier_mapping.get("default").cloned().or_else(|| {
        if cfg.tiers.contains_key("tier3") {
            Some("tier3".to_string())
        } else {
            cfg.tiers
                .keys()
                .find(|name| name.starts_with("tier-3-") || name.starts_with("tier3"))
                .cloned()
        }
    })
}

fn resolve_strategy_tier_name(
    config: Option<&ProjectConfig>,
    model_spec: Option<&str>,
    tier: Option<&str>,
    force: bool,
    selection_without_explicit_tool: bool,
) -> Option<String> {
    if let Some(tier_name) = tier {
        return config
            .and_then(|cfg| cfg.resolve_tier_selector(tier_name))
            .or_else(|| Some(tier_name.to_string()));
    }

    if selection_without_explicit_tool && model_spec.is_none() && !force {
        return resolve_default_tier_name(config);
    }

    None
}

/// Resolve the initial tool based on the `ToolSelectionStrategy`.
///
/// Encapsulates the `Explicit`, `AnyAvailable`, `HeterogeneousPreferred`, and
/// `HeterogeneousStrict` resolution arms, keeping the main `handle_run` function
/// focused on orchestration.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_tool_by_strategy(
    strategy: &ToolSelectionStrategy,
    model_spec: Option<&str>,
    model: Option<&str>,
    thinking: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    project_root: &Path,
    force: bool,
    force_override_user_config: bool,
    needs_edit: bool,
    tier: Option<&str>,
    force_ignore_tier_setting: bool,
) -> Result<StrategyResolution> {
    let catalog = csa_config::EffectiveModelCatalog::shipped()?;
    resolve_tool_by_strategy_with_catalog(
        strategy,
        model_spec,
        model,
        thinking,
        config,
        global_config,
        &catalog,
        project_root,
        force,
        force_override_user_config,
        needs_edit,
        tier,
        force_ignore_tier_setting,
    )
}

/// Resolve the initial tool using the command's effective model catalog.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_tool_by_strategy_with_catalog(
    strategy: &ToolSelectionStrategy,
    model_spec: Option<&str>,
    model: Option<&str>,
    thinking: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    model_catalog: &csa_config::EffectiveModelCatalog,
    project_root: &Path,
    force: bool,
    force_override_user_config: bool,
    needs_edit: bool,
    tier: Option<&str>,
    force_ignore_tier_setting: bool,
) -> Result<StrategyResolution> {
    crate::run_helpers::validate_model_spec_tier_conflict(model_spec, tier, "run")?;
    let tier_bypass_allowed = crate::run_helpers::tier_bypass_allowed(config, global_config, false);
    match strategy {
        ToolSelectionStrategy::Explicit(t) => {
            let (tool, ms, m) = resolve_tool_and_model(crate::run_helpers::RoutingRequest {
                tool: Some(*t),
                model_spec,
                model,
                thinking,
                config,
                global_config: Some(global_config),
                model_catalog: Some(model_catalog),
                force,
                force_override_user_config,
                needs_edit,
                tier,
                force_ignore_tier_setting,
                tier_bypass_allowed,
                tool_is_auto_resolved: false,
                ..crate::run_helpers::RoutingRequest::new(project_root)
            })?;
            Ok(StrategyResolution {
                tool,
                model_spec: ms,
                model: m,
                resolved_tier_name: resolve_strategy_tier_name(
                    config, model_spec, tier, force, false,
                ),
                runtime_fallback_candidates: Vec::new(),
            })
        }
        ToolSelectionStrategy::AnyAvailable => {
            let (tool, ms, m) = resolve_tool_and_model(crate::run_helpers::RoutingRequest {
                model_spec,
                model,
                thinking,
                config,
                global_config: Some(global_config),
                model_catalog: Some(model_catalog),
                force,
                force_override_user_config,
                needs_edit,
                tier,
                force_ignore_tier_setting,
                tier_bypass_allowed,
                tool_is_auto_resolved: true,
                ..crate::run_helpers::RoutingRequest::new(project_root)
            })?;
            Ok(StrategyResolution {
                tool,
                model_spec: ms,
                model: m,
                resolved_tier_name: resolve_strategy_tier_name(
                    config, model_spec, tier, force, true,
                ),
                runtime_fallback_candidates: Vec::new(),
            })
        }
        ToolSelectionStrategy::HeterogeneousPreferred => resolve_heterogeneous_preferred(
            model_spec,
            model,
            thinking,
            config,
            global_config,
            model_catalog,
            project_root,
            force,
            force_override_user_config,
            needs_edit,
            tier,
            force_ignore_tier_setting,
        ),
        ToolSelectionStrategy::HeterogeneousStrict => {
            let res = resolve_heterogeneous_strict(
                model_spec,
                model,
                thinking,
                config,
                global_config,
                model_catalog,
                project_root,
                force,
                force_override_user_config,
                needs_edit,
                tier,
                force_ignore_tier_setting,
            )?;
            Ok(StrategyResolution {
                tool: res.0,
                model_spec: res.1,
                model: res.2,
                resolved_tier_name: resolve_strategy_tier_name(
                    config, model_spec, tier, force, false,
                ),
                runtime_fallback_candidates: Vec::new(),
            })
        }
    }
}

fn collect_enabled_tools(
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    model_hint: Option<&str>,
) -> Vec<ToolName> {
    if let Some(cfg) = config {
        let tools: Vec<_> = csa_config::global::routing_candidate_tools()
            .iter()
            .filter(|t| {
                let extra_env = global_config
                    .build_execution_env(t.as_str(), csa_config::ExecutionEnvOptions::default());
                cfg.is_tool_auto_selectable(t.as_str())
                    && crate::run_helpers::is_tool_runtime_available_for_config_with_env(
                        t.as_str(),
                        Some(cfg),
                        model_hint,
                        extra_env.as_ref(),
                    )
            })
            .copied()
            .collect();
        csa_config::global::sort_tools_by_effective_priority(&tools, Some(cfg), global_config)
    } else {
        Vec::new()
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_heterogeneous_preferred(
    model_spec: Option<&str>,
    model: Option<&str>,
    thinking: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    model_catalog: &csa_config::EffectiveModelCatalog,
    project_root: &Path,
    force: bool,
    force_override_user_config: bool,
    needs_edit: bool,
    tier: Option<&str>,
    force_ignore_tier_setting: bool,
) -> Result<StrategyResolution> {
    let detected_parent_tool = detect_parent_tool();
    let parent_tool_name = resolve_tool(detected_parent_tool, global_config);
    let tier_bypass_allowed = crate::run_helpers::tier_bypass_allowed(config, global_config, false);

    if let Some(parent_str) = parent_tool_name.as_deref() {
        let parent_tool = parse_tool_name(parent_str)?;
        let enabled_tools = collect_enabled_tools(config, global_config, model_spec);
        let heterogeneous_candidates =
            resolve_heterogeneous_candidates(&parent_tool, &enabled_tools);

        match heterogeneous_candidates.first().copied() {
            Some(tool) => {
                let fallback = if model_spec.is_some() {
                    Vec::new()
                } else {
                    heterogeneous_candidates.into_iter().skip(1).collect()
                };
                let (t, ms, m) = resolve_tool_and_model(crate::run_helpers::RoutingRequest {
                    tool: Some(tool),
                    model_spec,
                    model,
                    thinking,
                    config,
                    global_config: Some(global_config),
                    model_catalog: Some(model_catalog),
                    force,
                    force_override_user_config,
                    needs_edit,
                    tier,
                    force_ignore_tier_setting,
                    tier_bypass_allowed,
                    tool_is_auto_resolved: true,
                    ..crate::run_helpers::RoutingRequest::new(project_root)
                })?;
                Ok(StrategyResolution {
                    tool: t,
                    model_spec: ms,
                    model: m,
                    resolved_tier_name: resolve_strategy_tier_name(
                        config, model_spec, tier, force, false,
                    ),
                    runtime_fallback_candidates: fallback,
                })
            }
            None => {
                warn!(
                    "No heterogeneous tool available (parent: {}, family: {}). Falling back to any available tool.",
                    parent_tool.as_str(),
                    parent_tool.model_family()
                );
                let (t, ms, m) = resolve_tool_and_model(crate::run_helpers::RoutingRequest {
                    model_spec,
                    model,
                    thinking,
                    config,
                    global_config: Some(global_config),
                    model_catalog: Some(model_catalog),
                    force,
                    force_override_user_config,
                    needs_edit,
                    tier,
                    force_ignore_tier_setting,
                    tier_bypass_allowed,
                    tool_is_auto_resolved: true,
                    ..crate::run_helpers::RoutingRequest::new(project_root)
                })?;
                Ok(StrategyResolution {
                    tool: t,
                    model_spec: ms,
                    model: m,
                    resolved_tier_name: resolve_strategy_tier_name(
                        config, model_spec, tier, force, true,
                    ),
                    runtime_fallback_candidates: Vec::new(),
                })
            }
        }
    } else {
        warn!(
            "HeterogeneousPreferred requested but no parent tool context/defaults.tool found. Falling back to AnyAvailable."
        );
        let (t, ms, m) = resolve_tool_and_model(crate::run_helpers::RoutingRequest {
            model_spec,
            model,
            thinking,
            config,
            global_config: Some(global_config),
            model_catalog: Some(model_catalog),
            force,
            force_override_user_config,
            needs_edit,
            tier,
            force_ignore_tier_setting,
            tier_bypass_allowed,
            tool_is_auto_resolved: true,
            ..crate::run_helpers::RoutingRequest::new(project_root)
        })?;
        Ok(StrategyResolution {
            tool: t,
            model_spec: ms,
            model: m,
            resolved_tier_name: resolve_strategy_tier_name(config, model_spec, tier, force, true),
            runtime_fallback_candidates: Vec::new(),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_heterogeneous_strict(
    model_spec: Option<&str>,
    model: Option<&str>,
    thinking: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    model_catalog: &csa_config::EffectiveModelCatalog,
    project_root: &Path,
    force: bool,
    force_override_user_config: bool,
    needs_edit: bool,
    tier: Option<&str>,
    force_ignore_tier_setting: bool,
) -> Result<(ToolName, Option<String>, Option<String>)> {
    let detected_parent_tool = detect_parent_tool();
    let parent_tool_name = resolve_tool(detected_parent_tool, global_config);
    let tier_bypass_allowed = crate::run_helpers::tier_bypass_allowed(config, global_config, false);

    if let Some(parent_str) = parent_tool_name.as_deref() {
        let parent_tool = parse_tool_name(parent_str)?;
        let enabled_tools = collect_enabled_tools(config, global_config, model_spec);

        match csa_config::global::select_heterogeneous_tool(&parent_tool, &enabled_tools) {
            Some(tool) => resolve_tool_and_model(crate::run_helpers::RoutingRequest {
                tool: Some(tool),
                model_spec,
                model,
                thinking,
                config,
                global_config: Some(global_config),
                model_catalog: Some(model_catalog),
                force,
                force_override_user_config,
                needs_edit,
                tier,
                force_ignore_tier_setting,
                tier_bypass_allowed,
                tool_is_auto_resolved: true,
                ..crate::run_helpers::RoutingRequest::new(project_root)
            }),
            None => {
                anyhow::bail!(
                    "No heterogeneous tool available (parent: {}, family: {}).\n\n\
                     If this is a low-risk task (exploration, documentation, code reading),\n\
                     consider using `--tool any-available` instead.",
                    parent_tool.as_str(),
                    parent_tool.model_family()
                );
            }
        }
    } else {
        warn!(
            "HeterogeneousStrict requested but no parent tool context/defaults.tool found. Falling back to AnyAvailable."
        );
        resolve_tool_and_model(crate::run_helpers::RoutingRequest {
            model_spec,
            model,
            thinking,
            config,
            global_config: Some(global_config),
            model_catalog: Some(model_catalog),
            force,
            force_override_user_config,
            needs_edit,
            tier,
            force_ignore_tier_setting,
            tier_bypass_allowed,
            tool_is_auto_resolved: true,
            ..crate::run_helpers::RoutingRequest::new(project_root)
        })
    }
}

/// Resolved skill, prompt text, and overridden CLI params.
#[path = "run_cmd_tool_selection_skill.rs"]
mod skill;
pub(crate) use skill::{
    SkillPromptSource, SkillResolution, build_skill_prompt_parts, resolve_return_target_session_id,
    resolve_skill_and_prompt,
};

#[cfg(test)]
#[path = "run_cmd_tool_selection_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "run_cmd_tool_selection_model_spec_tests.rs"]
mod model_spec_tests;

#[cfg(test)]
#[path = "run_cmd_tool_selection_hint_tests.rs"]
mod hint_tests;
