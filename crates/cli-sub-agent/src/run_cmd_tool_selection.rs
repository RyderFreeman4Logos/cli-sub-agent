//! Tool selection, session selection, and failover helpers for `csa run`.
//!
//! Extracted from `run_cmd.rs` to keep module sizes manageable.

use std::path::Path;

use anyhow::Result;
use tracing::warn;

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{ToolName, ToolSelectionStrategy};
use csa_session::{MetaSessionState, SessionPhase, resolve_session_prefix};

use crate::cli::ReturnTarget;
use crate::run_helpers::{
    detect_parent_tool, is_tool_binary_available, parse_tool_name, read_prompt, resolve_tool,
    resolve_tool_and_model,
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
    sorted_sessions.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
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
    /// Remaining heterogeneous candidates for runtime fallback (HeterogeneousPreferred only).
    pub(crate) runtime_fallback_candidates: Vec<ToolName>,
}

/// Resolve the initial tool based on the `ToolSelectionStrategy`.
///
/// Encapsulates the `Explicit`, `AnyAvailable`, `HeterogeneousPreferred`, and
/// `HeterogeneousStrict` resolution arms, keeping the main `handle_run` function
/// focused on orchestration.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_tool_by_strategy(
    strategy: &ToolSelectionStrategy,
    model_spec: Option<&str>,
    model: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    project_root: &Path,
    force: bool,
    force_override_user_config: bool,
) -> Result<StrategyResolution> {
    match strategy {
        ToolSelectionStrategy::Explicit(t) => {
            let (tool, ms, m) = resolve_tool_and_model(
                Some(*t),
                model_spec,
                model,
                config,
                project_root,
                force,
                force_override_user_config,
            )?;
            Ok(StrategyResolution {
                tool,
                model_spec: ms,
                model: m,
                runtime_fallback_candidates: Vec::new(),
            })
        }
        ToolSelectionStrategy::AnyAvailable => {
            let (tool, ms, m) = resolve_tool_and_model(
                None,
                model_spec,
                model,
                config,
                project_root,
                force,
                force_override_user_config,
            )?;
            Ok(StrategyResolution {
                tool,
                model_spec: ms,
                model: m,
                runtime_fallback_candidates: Vec::new(),
            })
        }
        ToolSelectionStrategy::HeterogeneousPreferred => resolve_heterogeneous_preferred(
            model_spec,
            model,
            config,
            global_config,
            project_root,
            force,
            force_override_user_config,
        ),
        ToolSelectionStrategy::HeterogeneousStrict => {
            let res = resolve_heterogeneous_strict(
                model_spec,
                model,
                config,
                global_config,
                project_root,
                force,
                force_override_user_config,
            )?;
            Ok(StrategyResolution {
                tool: res.0,
                model_spec: res.1,
                model: res.2,
                runtime_fallback_candidates: Vec::new(),
            })
        }
    }
}

fn collect_enabled_tools(
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Vec<ToolName> {
    if let Some(cfg) = config {
        let tools: Vec<_> = csa_config::global::all_known_tools()
            .iter()
            .filter(|t| {
                cfg.is_tool_auto_selectable(t.as_str()) && is_tool_binary_available(t.as_str())
            })
            .copied()
            .collect();
        csa_config::global::sort_tools_by_effective_priority(&tools, Some(cfg), global_config)
    } else {
        Vec::new()
    }
}

fn resolve_heterogeneous_preferred(
    model_spec: Option<&str>,
    model: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    project_root: &Path,
    force: bool,
    force_override_user_config: bool,
) -> Result<StrategyResolution> {
    let detected_parent_tool = detect_parent_tool();
    let parent_tool_name = resolve_tool(detected_parent_tool, global_config);

    if let Some(parent_str) = parent_tool_name.as_deref() {
        let parent_tool = parse_tool_name(parent_str)?;
        let enabled_tools = collect_enabled_tools(config, global_config);
        let heterogeneous_candidates =
            resolve_heterogeneous_candidates(&parent_tool, &enabled_tools);

        match heterogeneous_candidates.first().copied() {
            Some(tool) => {
                let fallback = heterogeneous_candidates.into_iter().skip(1).collect();
                let (t, ms, m) = resolve_tool_and_model(
                    Some(tool),
                    model_spec,
                    model,
                    config,
                    project_root,
                    force,
                    force_override_user_config,
                )?;
                Ok(StrategyResolution {
                    tool: t,
                    model_spec: ms,
                    model: m,
                    runtime_fallback_candidates: fallback,
                })
            }
            None => {
                warn!(
                    "No heterogeneous tool available (parent: {}, family: {}). Falling back to any available tool.",
                    parent_tool.as_str(),
                    parent_tool.model_family()
                );
                let (t, ms, m) = resolve_tool_and_model(
                    None,
                    model_spec,
                    model,
                    config,
                    project_root,
                    force,
                    force_override_user_config,
                )?;
                Ok(StrategyResolution {
                    tool: t,
                    model_spec: ms,
                    model: m,
                    runtime_fallback_candidates: Vec::new(),
                })
            }
        }
    } else {
        warn!(
            "HeterogeneousPreferred requested but no parent tool context/defaults.tool found. Falling back to AnyAvailable."
        );
        let (t, ms, m) = resolve_tool_and_model(
            None,
            model_spec,
            model,
            config,
            project_root,
            force,
            force_override_user_config,
        )?;
        Ok(StrategyResolution {
            tool: t,
            model_spec: ms,
            model: m,
            runtime_fallback_candidates: Vec::new(),
        })
    }
}

fn resolve_heterogeneous_strict(
    model_spec: Option<&str>,
    model: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    project_root: &Path,
    force: bool,
    force_override_user_config: bool,
) -> Result<(ToolName, Option<String>, Option<String>)> {
    let detected_parent_tool = detect_parent_tool();
    let parent_tool_name = resolve_tool(detected_parent_tool, global_config);

    if let Some(parent_str) = parent_tool_name.as_deref() {
        let parent_tool = parse_tool_name(parent_str)?;
        let enabled_tools = collect_enabled_tools(config, global_config);

        match csa_config::global::select_heterogeneous_tool(&parent_tool, &enabled_tools) {
            Some(tool) => resolve_tool_and_model(
                Some(tool),
                model_spec,
                model,
                config,
                project_root,
                force,
                force_override_user_config,
            ),
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
        resolve_tool_and_model(
            None,
            model_spec,
            model,
            config,
            project_root,
            force,
            force_override_user_config,
        )
    }
}

/// Resolved skill, prompt text, and overridden CLI params.
pub(crate) struct SkillResolution {
    pub(crate) prompt_text: String,
    pub(crate) resolved_skill: Option<ResolvedSkill>,
    pub(crate) tool: Option<csa_core::types::ToolArg>,
    pub(crate) model: Option<String>,
    pub(crate) thinking: Option<String>,
}

/// Resolve the skill (if any), build the prompt, and apply agent config
/// overrides for tool/model/thinking.
pub(crate) fn resolve_skill_and_prompt(
    skill: Option<&str>,
    prompt: Option<String>,
    tool: Option<csa_core::types::ToolArg>,
    model: Option<String>,
    thinking: Option<String>,
    project_root: &Path,
) -> Result<SkillResolution> {
    let resolved_skill = if let Some(skill_name) = skill {
        Some(skill_resolver::resolve_skill(skill_name, project_root)?)
    } else {
        None
    };

    let prompt_text = if let Some(ref sk) = resolved_skill {
        let mut parts = vec![sk.skill_md.clone()];

        // Load extra_context files relative to the skill directory.
        if let Some(agent) = sk.agent_config() {
            for extra in &agent.extra_context {
                let extra_path = sk.dir.join(extra);
                match std::fs::read_to_string(&extra_path) {
                    Ok(content) => {
                        parts.push(format!(
                            "<context-file path=\"{}\">\n{}\n</context-file>",
                            extra, content
                        ));
                    }
                    Err(e) => {
                        warn!(path = %extra, error = %e, "Failed to load skill extra_context file");
                    }
                }
            }
        }

        if let Some(user_prompt) = prompt {
            parts.push(format!("---\n\n{}", user_prompt));
        }

        parts.join("\n\n")
    } else {
        read_prompt(prompt)?
    };

    // Apply skill agent config overrides for tool/model when CLI didn't specify.
    let skill_agent = resolved_skill.as_ref().and_then(|sk| sk.agent_config());
    let tool = if tool.is_none() {
        skill_agent
            .and_then(|a| a.tools.first())
            .and_then(|t| parse_tool_name(&t.tool).ok())
            .map(csa_core::types::ToolArg::Specific)
            .or(tool)
    } else {
        tool
    };
    let model = if model.is_none() {
        skill_agent
            .and_then(|a| a.tools.first())
            .and_then(|t| t.model.clone())
            .or(model)
    } else {
        model
    };
    let thinking = if thinking.is_none() {
        skill_agent
            .and_then(|a| a.tools.first())
            .and_then(|t| t.thinking_budget.clone())
            .or(thinking)
    } else {
        thinking
    };

    Ok(SkillResolution {
        prompt_text,
        resolved_skill,
        tool,
        model,
        thinking,
    })
}

/// Resolve the `--return-to` target to a concrete session ID.
pub(crate) fn resolve_return_target_session_id(
    return_target: &ReturnTarget,
    project_root: &Path,
    fork_source_ref: Option<&str>,
    parent_flag: Option<&str>,
) -> Result<Option<String>> {
    match return_target {
        ReturnTarget::Last => {
            let sessions = csa_session::list_sessions(project_root, None)?;
            let (selected_id, _) = resolve_last_session_selection(sessions)?;
            Ok(Some(selected_id))
        }
        ReturnTarget::SessionId(session_ref) => {
            let resolved = resolve_session_reference(project_root, session_ref)?;
            Ok(Some(resolved))
        }
        ReturnTarget::Auto => {
            let env_parent = std::env::var("CSA_SESSION_ID").ok();
            let candidate = fork_source_ref
                .map(ToOwned::to_owned)
                .or_else(|| parent_flag.map(ToOwned::to_owned))
                .or(env_parent);

            if let Some(session_ref) = candidate {
                let resolved = resolve_session_reference(project_root, &session_ref)?;
                Ok(Some(resolved))
            } else {
                Ok(None)
            }
        }
    }
}
