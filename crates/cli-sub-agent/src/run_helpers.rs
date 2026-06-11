//! Helper functions for `csa run`: tool resolution, executor building, token parsing.

use anyhow::Result;

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_executor::ModelSpec;

#[path = "run_helpers_atomic_commit.rs"]
mod atomic_commit;
#[path = "run_helpers_compound_tier.rs"]
mod compound_tier;
#[path = "run_helpers_edit_requirement.rs"]
mod edit_requirement;
#[path = "run_helpers_executor.rs"]
mod executor;
#[path = "run_helpers_inline_review_context.rs"]
mod inline_review_context;
#[path = "run_helpers_model_compat.rs"]
pub(crate) mod model_compat;
#[path = "run_helpers_model_spec_validation.rs"]
mod model_spec_validation;
#[path = "run_helpers_prompt.rs"]
mod prompt;
#[path = "run_helpers_routing_conflict.rs"]
mod routing_conflict;
#[path = "run_helpers_routing_request.rs"]
mod routing_request;
#[path = "run_helpers_tier_bypass_gate.rs"]
mod tier_bypass_gate;
#[path = "run_helpers_tier_resolution.rs"]
mod tier_resolution;
#[path = "run_helpers_token_parse.rs"]
mod token_parse;
#[path = "run_helpers_tool_availability.rs"]
mod tool_availability;
#[cfg(test)]
pub(crate) use atomic_commit::atomic_commit_discipline_preamble;
pub(crate) use atomic_commit::prepend_atomic_commit_discipline_to_prompt;
pub(crate) use compound_tier::{apply_compound_tier_selector, apply_compound_tier_selector_arg};
pub(crate) use edit_requirement::{infer_task_edit_requirement, resolve_task_edit_requirement};
pub(crate) use executor::{build_executor, model_name_for_tier_validation};
pub(crate) use inline_review_context::prepend_review_context_to_prompt;
use model_spec_validation::enforce_model_spec_matches_tool_default;
#[cfg(test)]
pub(crate) use prompt::resolve_prompt_with_file_from_reader;
pub(crate) use prompt::{
    is_prompt_file_stdin_sentinel, read_prompt, resolve_positional_stdin_sentinel,
    resolve_prompt_with_file,
};
pub(crate) use routing_conflict::{is_routing_conflict, routing_conflict_error};
pub(crate) use routing_request::RoutingRequest;
pub(crate) use tier_bypass_gate::tier_bypass_allowed;
pub(crate) use tier_bypass_gate::{
    TierBypassGateCtx, TierBypassGateFlags, enforce_tier_bypass_gate,
};
pub(crate) use tier_resolution::{
    TierToolResolution, collect_available_tier_models, collect_preferred_tier_models,
    evaluate_tier_models, resolve_preferred_tool_from_tier,
    resolve_runtime_available_tier_fallback, resolve_tool_from_tier,
};
pub(crate) use token_parse::parse_token_usage;
#[cfg(test)]
pub(crate) use token_parse::{extract_cost, extract_number};
pub(crate) use tool_availability::{
    ToolBinaryAvailability, is_tool_binary_available_for_config,
    is_tool_runtime_available_for_config, resolved_claude_code_transport, resolved_codex_transport,
    resolved_tool_binary_name, tool_binary_availability, tool_runtime_availability,
};

#[cfg(test)]
pub(crate) const TEST_SKIP_TOOL_AVAILABILITY_CHECK_ENV: &str =
    "CSA_TEST_SKIP_TOOL_AVAILABILITY_CHECK";

#[cfg(test)]
pub(crate) const TEST_ASSUME_TOOLS_AVAILABLE_ENV: &str = "CSA_TEST_ASSUME_TOOLS_AVAILABLE";

/// Reject direct-tool routing that both uses and ignores tiers.
pub(crate) fn validate_tool_tier_override_flags(
    explicit_tool_requested: bool,
    tier: Option<&str>,
    force_ignore_tier_setting: bool,
) -> Result<()> {
    if explicit_tool_requested && tier.is_some() && force_ignore_tier_setting {
        return Err(routing_conflict_error(
            "Conflicting routing flags: --tool + --tier uses the tier's model/thinking for \
             that tool, while --force-ignore-tier-setting bypasses tier routing.\n\
             Remove --force-ignore-tier-setting to use tier routing, or remove --tier to \
             bypass tiers entirely.",
        ));
    }

    Ok(())
}

pub(crate) fn validate_direct_tool_tier_restriction(
    direct_tool_requested: bool,
    project_config: Option<&ProjectConfig>,
    effective_tier: Option<&str>,
    _force_override_user_config: bool,
    force_ignore_tier_setting: bool,
    model_spec_provided: bool,
) -> Result<()> {
    let Some(cfg) = project_config else {
        return Ok(());
    };
    let bypass_tier = force_ignore_tier_setting;
    if cfg.tiers.is_empty()
        || bypass_tier
        || effective_tier.is_some()
        || !direct_tool_requested
        || model_spec_provided
    {
        return Ok(());
    }

    let available: Vec<&str> = cfg.tiers.keys().map(|k| k.as_str()).collect();
    let alias_hint = cfg.format_tier_aliases();
    anyhow::bail!(
        "Direct --tool is restricted when tiers are configured. \
         Use --tier <name> to specify which tier's model/thinking config to use, \
         or --hint-difficulty <label> to route through [tier_mapping]. \
         Emergency direct-tool bypasses require \
         [tier_policy].allow_force_bypass = true in the global CSA config. \
         Available tiers: [{}]{alias_hint}",
        available.join(", ")
    );
}

pub(crate) fn validate_model_spec_tier_conflict(
    model_spec: Option<&str>,
    tier: Option<&str>,
    command: &str,
) -> Result<()> {
    if model_spec.is_some() && tier.is_some() {
        return Err(routing_conflict_error(format!(
            "Conflicting routing flags for `csa {command}`: --model-spec and --tier are mutually exclusive.\n\
             Use --model-spec for an exact `tool/provider/model/thinking` selection, or use --tier for tier-managed routing and failover."
        )));
    }

    Ok(())
}

/// Returns true when `--tier` is specified without an explicit `--tool`.
///
/// Auto-routing in this case may silently select the wrong tool for the task
/// (e.g., gemini-cli with allow_edit=false for write tasks).
pub(crate) fn tier_without_tool_should_warn(tier: Option<&str>, tool_explicitly_set: bool) -> bool {
    tier.is_some() && !tool_explicitly_set
}

/// Emit a warning when `--tier` is specified without `--tool`.
///
/// Backward-compatible: this is a warning only, not an error.
pub(crate) fn warn_if_tier_without_tool(tier: Option<&str>, tool_explicitly_set: bool) {
    if tier_without_tool_should_warn(tier, tool_explicitly_set) {
        tracing::warn!(
            tier = tier.unwrap_or(""),
            "--tier without --tool uses auto-routing; \
             specify --tool auto|claude-code|codex|gemini-cli to control tool selection"
        );
        eprintln!(
            "warning: --tier without --tool uses auto-routing; \
             specify --tool auto|claude-code|codex|gemini-cli to control tool selection"
        );
    }
}

/// Resolve tool and model from CLI args and config.
///
/// Returns (tool, model_spec, model) where:
/// - tool: the selected tool (from CLI or tier-based selection)
/// - model_spec: optional model spec string (from CLI or tier)
/// - model: optional model string (from CLI, with alias resolution applied)
///
/// When tool is None, uses tier-based round-robin selection.
/// `needs_edit`: when true, excludes tools with any write restriction (allow_edit_existing_files or allow_write_new_files false).
/// `tool_is_auto_resolved`: when true, the `tool` param was auto-selected (not user CLI),
///   so it should not trigger tier enforcement blocking.
pub(crate) fn resolve_tool_and_model(
    request: RoutingRequest<'_>,
) -> Result<(ToolName, Option<String>, Option<String>)> {
    // Destructure request at the top for clean access to all fields
    let RoutingRequest {
        tool,
        model_spec,
        model,
        thinking,
        config,
        project_root,
        force,
        force_override_user_config,
        needs_edit,
        tier,
        force_ignore_tier_setting,
        tier_bypass_allowed,
        tool_is_auto_resolved,
    } = request;
    let tiers_configured = config.is_some_and(|c| !c.tiers.is_empty());
    let bypass_tier = force_ignore_tier_setting || tier_bypass_allowed;
    let exact_selection_active = model_spec.is_some();

    // Enforce tier routing: block direct --tool/--model/--thinking when tiers are configured,
    // unless a shared tier-bypass gate has accepted an explicit bypass. --model-spec is
    // exact selection, but it must still match a configured tier model unless bypassed below.
    // Auto-resolved tools (from HeterogeneousPreferred etc.) don't count as user-explicit.
    let tool_triggers_enforcement = tool.is_some() && !tool_is_auto_resolved;
    validate_tool_tier_override_flags(tool_triggers_enforcement, tier, force_ignore_tier_setting)?;
    if tiers_configured
        && !bypass_tier
        && tier.is_none()
        && !exact_selection_active
        && (tool_triggers_enforcement || model.is_some() || thinking.is_some())
    {
        let cfg = config.unwrap();
        let mut tier_list = String::new();
        for name in cfg.tiers.keys() {
            if !tier_list.is_empty() {
                tier_list.push_str(", ");
            }
            tier_list.push_str(name);
        }
        let alias_hint = cfg.format_tier_aliases();
        anyhow::bail!(
            "Direct --tool/--model/--thinking is restricted when tiers are configured.\n\
             Use --tier <name> to specify which tier's model/thinking config to use, \
             or set [tier_policy].allow_force_bypass = true in the global CSA config \
             for emergency bypasses.\n\
             Available tiers: [{tier_list}]{alias_hint}\n\
             Hint: omit --tool entirely to use auto-selection, or use --tool auto"
        );
    }

    if force_ignore_tier_setting && tiers_configured && tier.is_none() && model_spec.is_none() {
        let configured_tool_defaults = tool.and_then(|tool_name| {
            config.map(|cfg| {
                (
                    cfg.tool_default_model(tool_name.as_str()).is_some(),
                    cfg.tool_default_thinking(tool_name.as_str()).is_some(),
                )
            })
        });
        let has_configured_default_model =
            configured_tool_defaults.is_some_and(|(has_model, _)| has_model);
        let has_configured_default_thinking =
            configured_tool_defaults.is_some_and(|(_, has_thinking)| has_thinking);

        let mut missing = Vec::new();
        if tool.is_none() {
            missing.push("--tool");
        }
        if model.is_none() && !has_configured_default_model {
            missing.push("--model");
        }
        if thinking.is_none() && !has_configured_default_thinking {
            missing.push("--thinking");
        }

        if !missing.is_empty() {
            anyhow::bail!(
                "When using --force-ignore-tier-setting to bypass tier enforcement, \
                 you must provide complete model specification.\n\
                 Missing required flags: {}\n\
                 Example: csa run --sa-mode <true|false> --force-ignore-tier-setting --tool claude-code \
                 --model claude-3-5-sonnet-20241022 --thinking medium \"prompt\"",
                missing.join(", ")
            );
        }
    }

    // Validate and canonicalize tier selector (accepts direct tier names and tier_mapping aliases).
    // Even in bypass_tier mode, resolve aliases so resolve_tool_from_tier gets a canonical name.
    let canonical_tier: Option<String> = if let Some(tier_name) = tier {
        if let Some(cfg) = config {
            if let Some(canonical) = cfg.resolve_tier_selector(tier_name) {
                Some(canonical)
            } else if bypass_tier {
                // bypass mode: tolerate unknown selector (pass through as-is)
                Some(tier_name.to_string())
            } else {
                let available: Vec<&str> = cfg.tiers.keys().map(|k| k.as_str()).collect();
                let alias_hint = cfg.format_tier_aliases();
                let suggest_hint = cfg
                    .suggest_tier(tier_name)
                    .map(|s| format!("\nDid you mean '{s}'?"))
                    .unwrap_or_default();
                anyhow::bail!(
                    "Tier selector '{}' not found.\n\
                     Available tiers: [{}]{alias_hint}{suggest_hint}",
                    tier_name,
                    available.join(", ")
                );
            }
        } else {
            anyhow::bail!(
                "Tier '{}' specified but no project config found. \
                 Run 'csa init --full' to create a config with tier definitions.",
                tier_name
            );
        }
    } else {
        None
    };

    // Case 0: --tier provided -> resolve tool/model from tier definition.
    // A user-explicit `--tool` is a soft preference inside the selected tier:
    // prefer matching candidates first, then keep the rest of the tier failover chain.
    if let Some(ref canonical_name) = canonical_tier
        && let Some(cfg) = config
    {
        let resolution = if let Some(requested_tool) = tool.filter(|_| !tool_is_auto_resolved) {
            let preference_order = [requested_tool.as_str().to_string()];
            resolve_preferred_tool_from_tier(canonical_name, cfg, None, &preference_order, &[])?
        } else if let Some(resolution) = resolve_tool_from_tier(canonical_name, cfg, None, &[], &[])
        {
            resolution
        } else {
            anyhow::bail!(
                "No available tool found in tier '{}'. Check that at least one tool \
                     in the tier is enabled and installed.",
                canonical_name
            );
        };

        // Flow resolved tool through existing enforcement checks.
        cfg.enforce_tool_enabled(resolution.tool.as_str(), force_override_user_config)?;
        if !force {
            cfg.enforce_tier_whitelist(resolution.tool.as_str(), Some(&resolution.model_spec))?;
        }
        let resolved_model = model.map(|m| {
            config
                .map(|cfg| cfg.resolve_alias(m))
                .unwrap_or_else(|| m.to_string())
        });
        return Ok((resolution.tool, Some(resolution.model_spec), resolved_model));
    }

    // Case 1: model_spec provided → parse it to get tool. --model-spec is an
    // exact selection, but configured tiers still whitelist allowed specs.
    if let Some(spec) = model_spec {
        let parsed = ModelSpec::parse(spec)?;
        let tool_name = parse_tool_name(&parsed.tool)?;
        if let Some(requested_tool) = tool.filter(|_| !tool_is_auto_resolved)
            && requested_tool != tool_name
        {
            return Err(routing_conflict_error(format!(
                "Conflicting routing flags: --tool {} does not match --model-spec {}.\n\
                 The model spec selects tool {}. Use a matching --tool value or omit --tool.",
                requested_tool.as_str(),
                spec,
                tool_name.as_str()
            )));
        }
        // Enforce tool enablement from user config
        if let Some(cfg) = config {
            cfg.enforce_tool_enabled(tool_name.as_str(), force_override_user_config)?;
            if !force && !bypass_tier {
                if cfg.tiers.is_empty() {
                    enforce_model_spec_matches_tool_default(cfg, &parsed, spec)?;
                } else {
                    cfg.enforce_tier_whitelist(tool_name.as_str(), Some(spec))?;
                }
            }
        }
        let resolved_model = model.map(|m| {
            config
                .map(|cfg| cfg.resolve_alias(m))
                .unwrap_or_else(|| m.to_string())
        });
        return Ok((tool_name, Some(spec.to_string()), resolved_model));
    }

    // Case 2: tool provided → use it with optional model (apply alias resolution)
    if let Some(tool_name) = tool {
        // Enforce tool enablement from user config
        if let Some(cfg) = config {
            cfg.enforce_tool_enabled(tool_name.as_str(), force_override_user_config)?;
        }
        let resolved_model = model.map(|m| {
            config
                .map(|cfg| cfg.resolve_alias(m))
                .unwrap_or_else(|| m.to_string())
        });
        // Enforce tier whitelist: tool must be in tiers; model name must match if provided
        if !force
            && !bypass_tier
            && let Some(cfg) = config
        {
            cfg.enforce_tier_whitelist(tool_name.as_str(), None)?;
            cfg.enforce_tier_model_name(
                tool_name.as_str(),
                model_name_for_tier_validation(resolved_model.as_deref()),
            )?;
        }
        // Catch known-incompatible model/tool combinations before spawning.
        if let Some(ref m) = resolved_model {
            let configured_default =
                config.and_then(|cfg| cfg.tool_default_model(tool_name.as_str()));
            model_compat::validate_tool_model_compat(tool_name, m, configured_default)?;
        }
        return Ok((tool_name, None, resolved_model));
    }

    // Case 3: no tool/model_spec; use tiers, or --force any enabled runtime.
    if force {
        for tool in csa_config::global::all_known_tools() {
            let name = tool.as_str();
            let enabled = config.is_none_or(|cfg| cfg.is_tool_enabled(name));
            if enabled && is_tool_runtime_available_for_config(name, config, None) {
                let tool_name = parse_tool_name(name)?;
                return Ok((tool_name, None, None));
            }
        }
        anyhow::bail!(
            "No installed and enabled tools found. Install at least one tool \
             (gemini-cli, opencode, codex, claude-code) or check enabled status."
        );
    }

    if let Some(cfg) = config {
        let resolved_model = model.map(|m| {
            config
                .map(|c| c.resolve_alias(m))
                .unwrap_or_else(|| m.to_string())
        });
        // Round-robin rotation; write-restriction errors propagate.
        match csa_scheduler::resolve_tier_tool_rotated(cfg, "default", project_root, needs_edit) {
            Ok(Some((s, spec)))
                if is_tool_runtime_available_for_config(&s, Some(cfg), Some(&spec)) =>
            {
                return Ok((parse_tool_name(&s)?, Some(spec), resolved_model));
            }
            Ok(Some((s, spec))) => {
                tracing::warn!(
                    tool = %s,
                    model_spec = %spec,
                    "Skipping rotated tier candidate because the tool is not executable"
                );
            }
            Err(e) if csa_scheduler::is_no_writable_tier_tool_error(&e) => return Err(e),
            _ => {}
        }
        // Fallback: original non-rotating selection, but keep runtime
        // availability aligned with the rotated path.
        if let Some(resolution) =
            resolve_runtime_available_tier_fallback(cfg, "default", needs_edit)
        {
            return Ok((resolution.tool, Some(resolution.model_spec), resolved_model));
        }
    }

    // Minimal configs with empty tiers may pick any auto-selectable runtime.
    if let Some(cfg) = config
        && cfg.tiers.is_empty()
    {
        for tool in csa_config::global::all_known_tools() {
            let name = tool.as_str();
            if cfg.is_tool_auto_selectable(name)
                && is_tool_runtime_available_for_config(name, Some(cfg), None)
            {
                let tool_name = parse_tool_name(name)?;
                return Ok((tool_name, None, None));
            }
        }
    }

    // Case 4: no config, no tier, and no auto-selectable installed tool → error
    anyhow::bail!(
        "No tool specified and no tier-based or auto-selectable tool available. \
         Use --tool, run 'csa init --full' to configure tiers, or install a tool."
    )
}

/// Check if a prompt is a context compress/compact command.
pub(crate) fn is_compress_command(prompt: &str) -> bool {
    let trimmed = prompt.trim();
    trimmed == "/compress" || trimmed == "/compact" || trimmed.starts_with("/compact ")
}

/// Parse a tool name string to ToolName enum.
pub(crate) fn parse_tool_name(name: &str) -> Result<ToolName> {
    match name {
        "gemini-cli" => Ok(ToolName::GeminiCli),
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        "claude-code" => Ok(ToolName::ClaudeCode),
        "openai-compat" => Ok(ToolName::OpenaiCompat),
        "antigravity-cli" => Ok(ToolName::AntigravityCli),
        _ => anyhow::bail!("Unknown tool: {name}"),
    }
}

/// Truncate a string to max_len characters, adding "..." if truncated.
///
/// Uses character (not byte) counting to safely handle multi-byte UTF-8.
pub(crate) fn truncate_prompt(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else {
        // Find byte offset for the character at position (max_len - 3)
        let truncate_at_chars = max_len.saturating_sub(3);
        let byte_offset = s
            .char_indices()
            .nth(truncate_at_chars)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        let substring = &s[..byte_offset];

        // Try to break at last space if possible
        if let Some(last_space) = substring.rfind(' ')
            && last_space > byte_offset / 2
        {
            return format!("{}...", &s[..last_space]);
        }

        format!("{substring}...")
    }
}

/// Detect the parent tool context.
///
/// Resolution order:
/// 1. `CSA_TOOL` environment variable (set by CSA when spawning children)
/// 2. `CSA_PARENT_TOOL` environment variable (set for grandchild processes)
/// 3. Process tree walking via `/proc` (Linux-only fallback)
pub(crate) fn detect_parent_tool() -> Option<String> {
    std::env::var("CSA_TOOL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("CSA_PARENT_TOOL")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(crate::process_tree::detect_ancestor_tool)
}

/// Resolve parent tool context using detection result with global-config fallback.
///
/// Resolution order:
/// 1. Detected parent tool from runtime context
/// 2. `~/.config/cli-sub-agent/config.toml` `[defaults].tool`
/// 3. None
///
/// The literal `"auto"` is the documented auto-select sentinel (see
/// `csa-config::tool_selection`), NOT a concrete tool name. It is normalized to
/// `None` here so callers that feed the result into `parse_tool_name` (the
/// heterogeneous strategy arms) treat "no concrete parent" rather than bailing
/// with `Unknown tool: auto`. Without this, a pinned SA-nested worker spawned in
/// a detached process tree (no ancestor tool detectable) and a global
/// `[defaults].tool = "auto"` would crash instead of honoring the inherited
/// `--model-spec` pin (#1741).
pub(crate) fn resolve_tool(detected: Option<String>, config: &GlobalConfig) -> Option<String> {
    detected
        .or_else(|| config.defaults.tool.clone())
        .filter(|tool| tool.trim() != "auto" && !tool.trim().is_empty())
}

#[cfg(test)]
#[path = "run_helpers_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "run_helpers_tests_prompt.rs"]
mod tests_prompt;

#[cfg(test)]
#[path = "run_helpers_tests_tail.rs"]
mod tests_tail;

#[cfg(test)]
#[path = "run_helpers_tier_tests.rs"]
mod tier_tests;

#[cfg(test)]
#[path = "run_helpers_tier_runtime_tests.rs"]
mod tier_runtime_tests;

#[cfg(test)]
#[path = "run_helpers_tier_force_tests.rs"]
mod tier_force_tests;

#[cfg(test)]
#[path = "run_helpers_transport_tests.rs"]
mod transport_tests;

#[cfg(test)]
#[path = "run_helpers_model_spec_tests.rs"]
mod model_spec_tests;

#[cfg(test)]
#[path = "run_helpers_compat_tests.rs"]
mod compat_tests;

#[cfg(test)]
#[path = "run_helpers_override_tests.rs"]
mod override_tests;

#[cfg(test)]
#[path = "run_helpers_inline_review_context_tests.rs"]
mod inline_review_context_tests;

#[cfg(test)]
#[path = "run_helpers_transport_integration_tests.rs"]
mod transport_integration_tests;
