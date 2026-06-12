//! Compound `--tier <tier>-<tool>` selector parsing for #1441.
//!
//! When the literal tier name doesn't match a configured tier, try parsing it
//! as a `<tier>-<tool>` compound. The prefix becomes the canonical tier name
//! and the suffix injects a tool override.

use anyhow::Result;

use csa_config::ProjectConfig;
use csa_core::types::{ToolArg, ToolName};

use crate::run_helpers::parse_tool_name;
use crate::run_helpers::routing_conflict_error;

fn resolve_alias_to_tool(alias_name: &str, cfg: &ProjectConfig) -> Option<ToolName> {
    let canonical = cfg.tool_aliases.get(alias_name)?;
    parse_tool_name(canonical).ok()
}

pub(crate) fn compound_tier_selects_tool(
    tier: Option<&str>,
    project_config: Option<&ProjectConfig>,
) -> bool {
    tier.zip(project_config).is_some_and(|(tier_str, cfg)| {
        cfg.resolve_tier_selector(tier_str).is_none()
            && cfg.try_parse_compound_tier_tool(tier_str).is_some()
    })
}

/// Apply compound `--tier <tier>-<tool>` parsing for `Option<ToolName>` callers.
///
/// Used by `csa review` and `csa debate` where the CLI `--tool` is typed as
/// `Option<ToolName>`. When `tier` is `Some` and the literal tier name does NOT
/// match a configured tier, try parsing it as `<tier>-<tool>` via
/// `ProjectConfig::try_parse_compound_tier_tool`. On success:
///   - Replace `tier` with the canonical tier name from the prefix.
///   - Inject the parsed tool when `tool` is `None`.
///   - Error when `tool` is already `Some(t)` and `t` differs from the parsed
///     tool.
///
/// Returns the original `(tier, tool)` unchanged when no compound parse fires.
pub(crate) fn apply_compound_tier_selector(
    tier: Option<String>,
    tool: Option<ToolName>,
    project_config: Option<&ProjectConfig>,
) -> Result<(Option<String>, Option<ToolName>)> {
    let Some(tier_str) = tier else {
        return Ok((None, tool));
    };
    let Some(cfg) = project_config else {
        return Ok((Some(tier_str), tool));
    };
    if cfg.resolve_tier_selector(&tier_str).is_some() {
        return Ok((Some(tier_str), tool));
    }
    let Some((canonical, parsed_tool)) = cfg.try_parse_compound_tier_tool(&tier_str) else {
        return Ok((Some(tier_str), tool));
    };

    if let Some(existing) = tool
        && existing != parsed_tool
    {
        return Err(routing_conflict_error(format!(
            "Conflicting routing flags: --tier '{tier_str}' parses as compound \
             (tier={canonical}, tool={parsed}), but --tool {existing_str} is also specified.\n\
             Use --tier {canonical} (with or without --tool {parsed}) to accept the compound's tool, \
             or use --tier {canonical} --tool {existing_str} to override the compound's tool with a \
             tier-internal selection.",
            parsed = parsed_tool.as_str(),
            existing_str = existing.as_str(),
        )));
    }

    tracing::info!(
        tier = %canonical,
        tool = parsed_tool.as_str(),
        original_selector = %tier_str,
        "Parsed compound tier selector: tier={canonical}, tool={}",
        parsed_tool.as_str(),
    );
    Ok((Some(canonical), Some(parsed_tool)))
}

/// Apply compound `--tier <tier>-<tool>` parsing for `Option<ToolArg>` callers.
///
/// Used by `csa run` where the CLI `--tool` is typed as `Option<ToolArg>`.
/// Semantics mirror `apply_compound_tier_selector`:
///   - `None` / `ToolArg::Auto` / `ToolArg::AnyAvailable` are upgraded to
///     `ToolArg::Specific(parsed_tool)` when compound parses.
///   - `ToolArg::Specific(t)` is left in place when `t == parsed_tool`, and
///     errors when `t != parsed_tool`.
///   - `ToolArg::Alias(name)` is resolved via `[tool_aliases]` for the
///     conflict check; if the alias cannot be resolved locally, defer to
///     downstream alias resolution and inject the compound tool.
pub(crate) fn apply_compound_tier_selector_arg(
    tier: Option<String>,
    tool: Option<ToolArg>,
    project_config: Option<&ProjectConfig>,
) -> Result<(Option<String>, Option<ToolArg>)> {
    let Some(tier_str) = tier else {
        return Ok((None, tool));
    };
    let Some(cfg) = project_config else {
        return Ok((Some(tier_str), tool));
    };
    if cfg.resolve_tier_selector(&tier_str).is_some() {
        return Ok((Some(tier_str), tool));
    }
    let Some((canonical, parsed_tool)) = cfg.try_parse_compound_tier_tool(&tier_str) else {
        return Ok((Some(tier_str), tool));
    };

    let merged = match tool {
        None | Some(ToolArg::Auto) | Some(ToolArg::AnyAvailable) => {
            Some(ToolArg::Specific(parsed_tool))
        }
        Some(ToolArg::Specific(t)) if t == parsed_tool => Some(ToolArg::Specific(t)),
        Some(ToolArg::Specific(t)) => {
            return Err(routing_conflict_error(format!(
                "Conflicting routing flags: --tier '{tier_str}' parses as compound \
                 (tier={canonical}, tool={parsed}), but --tool {existing_str} is also specified.\n\
                 Use --tier {canonical} (with or without --tool {parsed}) to accept the compound's tool, \
                 or use --tier {canonical} --tool {existing_str} to override the compound's tool with a \
                 tier-internal selection.",
                parsed = parsed_tool.as_str(),
                existing_str = t.as_str(),
            )));
        }
        Some(ToolArg::Alias(alias_name)) => match resolve_alias_to_tool(&alias_name, cfg) {
            Some(t) if t == parsed_tool => Some(ToolArg::Specific(t)),
            Some(t) => {
                return Err(routing_conflict_error(format!(
                    "Conflicting routing flags: --tier '{tier_str}' parses as compound \
                     (tier={canonical}, tool={parsed}), but --tool '{alias_name}' (alias for \
                     {existing_str}) is also specified.\n\
                     Use --tier {canonical} (with or without --tool {parsed}) to accept the compound's tool, \
                     or use --tier {canonical} --tool {existing_str} to override.",
                    parsed = parsed_tool.as_str(),
                    existing_str = t.as_str(),
                )));
            }
            None => Some(ToolArg::Specific(parsed_tool)),
        },
    };

    tracing::info!(
        tier = %canonical,
        tool = parsed_tool.as_str(),
        original_selector = %tier_str,
        "Parsed compound tier selector: tier={canonical}, tool={}",
        parsed_tool.as_str(),
    );
    Ok((Some(canonical), merged))
}

#[cfg(test)]
#[path = "run_helpers_compound_tier_tests.rs"]
mod tests;
