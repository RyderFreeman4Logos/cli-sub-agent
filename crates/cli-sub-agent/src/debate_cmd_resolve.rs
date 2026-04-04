//! Debate tool resolution logic.
//!
//! Extracted from `debate_cmd.rs` to stay under the 800-line monolith limit.

use std::path::Path;

use anyhow::{Context, Result};
use tracing::{debug, warn};

use super::debate_cmd::DebateMode;
use csa_config::global::{heterogeneous_counterpart, select_heterogeneous_tool};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;

/// Returns (tool, debate_mode, optional_model_spec). When tier resolves, model_spec is set.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_debate_tool(
    arg_tool: Option<ToolName>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    parent_tool: Option<&str>,
    project_root: &Path,
    force_override_user_config: bool,
    cli_tier: Option<&str>,
    force_ignore_tier_setting: bool,
) -> Result<(ToolName, DebateMode, Option<String>)> {
    let tiers_configured = project_config.is_some_and(|c| !c.tiers.is_empty());
    let bypass_tier = force_ignore_tier_setting || force_override_user_config;

    // Enforce tier routing: block direct --tool when tiers are configured,
    // unless --force-ignore-tier-setting (or --force-override-user-config) is active.
    if tiers_configured && !bypass_tier && cli_tier.is_none() && arg_tool.is_some() {
        let cfg = project_config.unwrap();
        let available: Vec<&str> = cfg.tiers.keys().map(|k| k.as_str()).collect();
        let alias_hint = cfg.format_tier_aliases();
        anyhow::bail!(
            "Direct --tool is restricted when tiers are configured. \
             Use --tier <name> or add --force-ignore-tier-setting to override. \
             Available tiers: [{}]{alias_hint}",
            available.join(", ")
        );
    }

    // CLI --tool override wins (when not blocked by tier enforcement above)
    if let Some(tool) = arg_tool {
        if let Some(cfg) = project_config {
            cfg.enforce_tool_enabled(tool.as_str(), force_override_user_config)?;
        }
        return Ok((tool, DebateMode::Heterogeneous, None));
    }

    let tier_name = resolve_debate_tier_name(
        project_config,
        global_config,
        cli_tier,
        force_override_user_config,
        force_ignore_tier_setting,
    )?;

    // Compute effective whitelist from tool selection (project > global)
    let effective_whitelist = project_config
        .and_then(|cfg| cfg.debate.as_ref())
        .map(|d| &d.tool)
        .unwrap_or(&global_config.debate.tool);
    let whitelist = effective_whitelist.whitelist();

    if let Some(ref tier) = tier_name {
        if let Some(cfg) = project_config
            && let Some(resolution) =
                crate::run_helpers::resolve_tool_from_tier(tier, cfg, parent_tool, whitelist, &[])
        {
            return Ok((
                resolution.tool,
                DebateMode::Heterogeneous,
                Some(resolution.model_spec),
            ));
        }
        // Tier set but no available tool found — fall through to tool-based resolution.
        if whitelist.is_some() {
            warn!(
                tier = %tier,
                "Tier '{}' has no tools matching [debate].tool whitelist — \
                 falling through to whitelist-based auto-selection (tier constraint bypassed)",
                tier
            );
        } else {
            debug!(
                tier = %tier,
                "Tier set but no available tool found, falling through to tool-based resolution"
            );
        }
    }

    // Project-level [debate] config override
    if let Some(project_debate) = project_config.and_then(|cfg| cfg.debate.as_ref()) {
        return resolve_debate_tool_from_selection(
            &project_debate.tool,
            parent_tool,
            project_config,
            global_config,
            project_root,
        )
        .map(|(t, m)| (t, m, None))
        .with_context(|| {
            format!(
                "Failed to resolve debate tool from project config: {}",
                ProjectConfig::config_path(project_root).display()
            )
        });
    }

    // Global config [debate] section
    resolve_debate_tool_from_selection(
        &global_config.debate.tool,
        parent_tool,
        project_config,
        global_config,
        project_root,
    )
    .map(|(t, m)| (t, m, None))
}

pub(crate) fn resolve_debate_tier_name(
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    cli_tier: Option<&str>,
    force_override_user_config: bool,
    force_ignore_tier_setting: bool,
) -> Result<Option<String>> {
    let bypass_tier = force_ignore_tier_setting || force_override_user_config;

    if let Some(cli) = cli_tier {
        if let Some(cfg) = project_config {
            if let Some(canonical) = cfg.resolve_tier_selector(cli) {
                return Ok(Some(canonical));
            }
            if bypass_tier {
                return Ok(Some(cli.to_string()));
            }
            let available: Vec<&str> = cfg.tiers.keys().map(|k| k.as_str()).collect();
            let alias_hint = cfg.format_tier_aliases();
            let suggest_hint = cfg
                .suggest_tier(cli)
                .map(|s| format!("\nDid you mean '{s}'?"))
                .unwrap_or_default();
            anyhow::bail!(
                "Tier selector '{}' not found.\n\
                 Available tiers: [{}]{alias_hint}{suggest_hint}",
                cli,
                available.join(", ")
            );
        }
        return Ok(Some(cli.to_string()));
    }

    Ok(project_config
        .and_then(|cfg| cfg.debate.as_ref())
        .and_then(|d| d.tier.as_deref())
        .or(global_config.debate.tier.as_deref())
        .map(|s| s.to_string()))
}

fn resolve_debate_tool_from_selection(
    selection: &csa_config::ToolSelection,
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    project_root: &Path,
) -> Result<(ToolName, DebateMode)> {
    // Single direct tool (not "auto")
    if let Some(tool_name) = selection.as_single() {
        let tool = crate::run_helpers::parse_tool_name(tool_name).map_err(|_| {
            anyhow::anyhow!(
                "Invalid [debate].tool value '{tool_name}'. Supported values: auto, gemini-cli, opencode, codex, claude-code."
            )
        })?;
        // Verify the tool is enabled in the project config
        if let Some(cfg) = project_config
            && !cfg.is_tool_enabled(tool_name)
        {
            anyhow::bail!(
                "[debate].tool = '{tool_name}' is disabled in project config. \
                     Enable it in [tools.{tool_name}] or change [debate].tool."
            );
        }
        return Ok((tool, DebateMode::Heterogeneous));
    }

    // Auto or whitelist — try heterogeneous auto-selection with optional filter
    let whitelist = selection.whitelist();
    if let Some(tool) =
        select_auto_debate_tool(parent_tool, project_config, global_config, whitelist)
    {
        return Ok((tool, DebateMode::Heterogeneous));
    }

    // Legacy counterpart fallback (only for true auto, not whitelist)
    if whitelist.is_none()
        && let Some(resolved) = parent_tool.and_then(heterogeneous_counterpart)
    {
        let counterpart_enabled = project_config.is_none_or(|cfg| cfg.is_tool_enabled(resolved));
        if counterpart_enabled {
            let tool = crate::run_helpers::parse_tool_name(resolved).map_err(|_| {
                anyhow::anyhow!(
                    "BUG: auto debate tool resolution returned invalid tool '{resolved}'"
                )
            })?;
            return Ok((tool, DebateMode::Heterogeneous));
        }
    }

    // All heterogeneous methods failed — try same-model fallback (whitelist-aware).
    // Same-model fallback is allowed when parent tool is in the whitelist
    // (user explicitly listed it, so using it as both proposer and critic is OK).
    if let Some(wl) = whitelist {
        let parent_in_whitelist = parent_tool.is_some_and(|pt| wl.iter().any(|w| w == pt));
        if !parent_in_whitelist {
            anyhow::bail!(
                "No tools from [debate].tool whitelist [{}] are available for \
                 heterogeneous debate, and parent tool '{}' is not in the whitelist \
                 for same-model fallback.",
                wl.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
                parent_tool.unwrap_or("<none>")
            );
        }
    }
    resolve_same_model_fallback(parent_tool, project_config, global_config, project_root)
}

fn select_auto_debate_tool(
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    whitelist: Option<&[String]>,
) -> Option<ToolName> {
    let parent_str = parent_tool?;
    let parent_tool_name = crate::run_helpers::parse_tool_name(parent_str).ok()?;
    let enabled_tools: Vec<_> = if let Some(cfg) = project_config {
        let tools: Vec<_> = csa_config::global::all_known_tools()
            .iter()
            .filter(|t| cfg.is_tool_auto_selectable(t.as_str()))
            .filter(|t| whitelist.is_none_or(|wl| wl.iter().any(|w| w == t.as_str())))
            .copied()
            .collect();
        csa_config::global::sort_tools_by_effective_priority(&tools, project_config, global_config)
    } else {
        let all = csa_config::global::all_known_tools();
        let tools: Vec<_> = all
            .iter()
            .filter(|t| whitelist.is_none_or(|wl| wl.iter().any(|w| w == t.as_str())))
            .copied()
            .collect();
        csa_config::global::sort_tools_by_effective_priority(&tools, project_config, global_config)
    };

    select_heterogeneous_tool(&parent_tool_name, &enabled_tools)
}

/// Attempt same-model adversarial fallback when heterogeneous selection fails.
///
/// Uses the parent tool (or any available tool) to run two independent sub-agents
/// as Proposer and Critic. Returns `SameModelAdversarial` mode to annotate output.
///
/// Fails with the standard auto-resolution error when:
/// - `same_model_fallback` is disabled in config
/// - No parent tool is detected and no tools are available
fn resolve_same_model_fallback(
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    project_root: &Path,
) -> Result<(ToolName, DebateMode)> {
    if !global_config.debate.same_model_fallback {
        return Err(debate_auto_resolution_error(parent_tool, project_root));
    }

    // Use the parent tool itself for same-model adversarial debate,
    // but only if the tool is enabled in project config.
    if let Some(parent_str) = parent_tool
        && let Ok(tool) = crate::run_helpers::parse_tool_name(parent_str)
    {
        let enabled = project_config
            .map(|cfg| cfg.is_tool_enabled(tool.as_str()))
            .unwrap_or(true);
        if enabled {
            return Ok((tool, DebateMode::SameModelAdversarial));
        }
    }

    // No usable parent tool — select the first enabled tool from project config
    let candidates: Vec<_> = if let Some(cfg) = project_config {
        csa_config::global::all_known_tools()
            .iter()
            .filter(|t| cfg.is_tool_enabled(t.as_str()))
            .copied()
            .collect()
    } else {
        csa_config::global::all_known_tools().to_vec()
    };
    // Prefer a tool that is both enabled AND installed on this system.
    // Fall back to first enabled tool if none are installed (preserves prior behavior).
    let installed = candidates
        .iter()
        .find(|t| crate::run_helpers::is_tool_binary_available(t.as_str()));
    if let Some(tool) = installed.or(candidates.first()) {
        return Ok((*tool, DebateMode::SameModelAdversarial));
    }

    Err(debate_auto_resolution_error(parent_tool, project_root))
}

fn debate_auto_resolution_error(parent_tool: Option<&str>, project_root: &Path) -> anyhow::Error {
    let parent = parent_tool.unwrap_or("<none>").escape_default().to_string();
    let global_path = GlobalConfig::config_path()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.config/cli-sub-agent/config.toml".to_string());
    let project_path = ProjectConfig::config_path(project_root)
        .display()
        .to_string();

    anyhow::anyhow!(
        "AUTO debate tool selection failed (tool = \"auto\").\n\n\
STOP: Do not proceed. Ask the user to configure the debate tool explicitly.\n\n\
Parent tool context: {parent}\n\
Supported auto mapping: claude-code <-> codex\n\n\
Choose one:\n\
1) Global config (user-level): {global_path}\n\
   [debate]\n\
   tool = \"codex\"  # or \"claude-code\", \"opencode\", \"gemini-cli\"\n\
2) Project config override: {project_path}\n\
   [debate]\n\
   tool = \"codex\"  # or \"claude-code\", \"opencode\", \"gemini-cli\"\n\
3) CLI override: csa debate --sa-mode <true|false> --tool codex\n\n\
Reason: CSA enforces heterogeneity in auto mode and will not fall back."
    )
}
