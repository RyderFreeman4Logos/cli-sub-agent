//! Debate tool resolution logic.
//!
//! Extracted from `debate_cmd.rs` to stay under the 800-line monolith limit.

use std::path::Path;

use super::debate_cmd::DebateMode;
use anyhow::{Context, Result};
use csa_config::global::{heterogeneous_counterpart, select_heterogeneous_tool};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;

/// Returns (tool, debate_mode, optional_model_spec). When tier resolves, model_spec is set.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_debate_tool(
    arg_tool: Option<ToolName>,
    arg_model_spec: Option<&str>,
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

    crate::run_helpers::validate_tool_tier_override_flags(
        arg_tool.is_some(),
        cli_tier,
        force_ignore_tier_setting,
    )?;
    crate::run_helpers::validate_model_spec_tier_conflict(arg_model_spec, cli_tier, "debate")?;

    if let Some(model_spec) = arg_model_spec {
        let (tool, resolved_model_spec, _) = crate::run_helpers::resolve_tool_and_model(
            arg_tool,
            Some(model_spec),
            None,
            project_config,
            project_root,
            false,
            force_override_user_config,
            false,
            cli_tier,
            force_ignore_tier_setting,
            false,
        )?;
        return Ok((tool, DebateMode::Heterogeneous, resolved_model_spec));
    }

    // Enforce tier routing: block direct --tool when tiers are configured,
    // unless --force-ignore-tier-setting (or --force-override-user-config) is active.
    if tiers_configured && !bypass_tier && cli_tier.is_none() && arg_tool.is_some() {
        let cfg = project_config.unwrap();
        let available: Vec<&str> = cfg.tiers.keys().map(|k| k.as_str()).collect();
        let alias_hint = cfg.format_tier_aliases();
        anyhow::bail!(
            "Direct --tool is restricted when tiers are configured. \
             Use --tier <name> to specify which tier's model/thinking config to use, \
             or add --force-ignore-tier-setting to override. \
             Available tiers: [{}]{alias_hint}",
            available.join(", ")
        );
    }

    let tier_name = resolve_debate_tier_name(
        project_config,
        global_config,
        cli_tier,
        force_override_user_config,
        force_ignore_tier_setting,
    )?;

    if let Some(tool) = arg_tool {
        if let Some(ref tier) = tier_name
            && let Some(cfg) = project_config
        {
            let resolution = crate::run_helpers::resolve_requested_tool_from_tier(
                tier,
                cfg,
                parent_tool,
                tool,
                force_override_user_config,
                &[],
            )?;
            return Ok((
                resolution.tool,
                DebateMode::Heterogeneous,
                Some(resolution.model_spec),
            ));
        }

        if let Some(cfg) = project_config {
            cfg.enforce_tool_enabled(tool.as_str(), force_override_user_config)?;
        }
        return Ok((tool, DebateMode::Heterogeneous, None));
    }

    // Compute effective whitelist from tool selection (project > global).
    // IMPORTANT (#648): When [debate].tool is set, it acts as a whitelist filter
    // on the tier's model list, silently narrowing a multi-tool tier to only the
    // specified tool(s). To use the full tier fallback chain, set tool = "auto".
    let effective_whitelist = project_config
        .and_then(|cfg| cfg.debate.as_ref())
        .map(|d| &d.tool)
        .unwrap_or(&global_config.debate.tool);
    let whitelist = effective_whitelist.whitelist();

    if let Some(ref tier) = tier_name {
        let cfg = project_config.ok_or_else(|| {
            anyhow::anyhow!(
                "Debate tier '{}' is configured, but no tier definitions are available. \
                 Run `csa init --full` or define [tiers.*] in config.",
                tier
            )
        })?;

        let tier_tools = cfg.list_tools_in_tier(tier);
        if let Some(wl) = whitelist {
            let matching_tools: Vec<&str> = tier_tools
                .iter()
                .filter(|(tool_name, _)| wl.iter().any(|allowed| allowed == tool_name))
                .map(|(tool_name, _)| tool_name.as_str())
                .collect();
            if matching_tools.is_empty() {
                let tier_tool_names: Vec<&str> = tier_tools
                    .iter()
                    .map(|(tool_name, _)| tool_name.as_str())
                    .collect();
                anyhow::bail!(
                    "Tier '{}' has no tools matching [debate].tool whitelist [{}]. \
                     The active debate tier remains authoritative.\n\
                     Tier tools: [{}].\n\
                     Update [debate].tool or choose a different tier.",
                    tier,
                    wl.join(", "),
                    tier_tool_names.join(", ")
                );
            }
        }

        if let Some(resolution) =
            crate::run_helpers::resolve_tool_from_tier(tier, cfg, parent_tool, whitelist, &[])
        {
            return Ok((
                resolution.tool,
                DebateMode::Heterogeneous,
                Some(resolution.model_spec),
            ));
        }

        let filtered_tools =
            crate::run_helpers::collect_available_tier_models(tier, cfg, whitelist, &[]);
        let configured_tools: Vec<&str> = tier_tools
            .iter()
            .map(|(tool_name, _)| tool_name.as_str())
            .collect();
        let available_tools: Vec<&str> = filtered_tools
            .iter()
            .map(|resolution| resolution.tool.as_str())
            .collect();
        anyhow::bail!(
            "Tier '{}' resolved for debate, but none of its tools are currently available.\n\
             Configured tier tools: [{}].\n\
             Available tier tools after enablement/install checks: [{}].",
            tier,
            configured_tools.join(", "),
            available_tools.join(", ")
        );
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

pub(crate) fn resolve_debate_model(
    cli_model: Option<&str>,
    config_model: Option<&str>,
    model_spec_active: bool,
) -> Option<String> {
    cli_model.map(str::to_string).or_else(|| {
        (!model_spec_active)
            .then_some(config_model)
            .flatten()
            .map(str::to_string)
    })
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
        let counterpart_available =
            crate::run_helpers::is_tool_binary_available_for_config(resolved, project_config);
        if counterpart_enabled && counterpart_available {
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
            .filter(|t| {
                crate::run_helpers::is_tool_binary_available_for_config(t.as_str(), project_config)
            })
            .filter(|t| whitelist.is_none_or(|wl| wl.iter().any(|w| w == t.as_str())))
            .copied()
            .collect();
        csa_config::global::sort_tools_by_effective_priority(&tools, project_config, global_config)
    } else {
        let all = csa_config::global::all_known_tools();
        let tools: Vec<_> = all
            .iter()
            .filter(|t| {
                crate::run_helpers::is_tool_binary_available_for_config(t.as_str(), project_config)
            })
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
    let installed = candidates.iter().find(|t| {
        crate::run_helpers::is_tool_binary_available_for_config(t.as_str(), project_config)
    });
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
