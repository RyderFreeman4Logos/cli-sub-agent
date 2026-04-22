//! Helper functions for `csa run`: tool resolution, executor building, token parsing.

use anyhow::{Context, Result};
use std::path::Path;

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_executor::{Executor, ModelSpec, ThinkingBudget};
use csa_session::TokenUsage;

#[path = "run_helpers_atomic_commit.rs"]
mod atomic_commit;
#[path = "run_helpers_edit_requirement.rs"]
mod edit_requirement;
#[path = "run_helpers_inline_review_context.rs"]
mod inline_review_context;
#[path = "run_helpers_prompt.rs"]
mod prompt;
#[path = "run_helpers_routing_conflict.rs"]
mod routing_conflict;
#[path = "run_helpers_tool_availability.rs"]
mod tool_availability;

#[cfg(test)]
pub(crate) use atomic_commit::atomic_commit_discipline_preamble;
pub(crate) use atomic_commit::prepend_atomic_commit_discipline_to_prompt;
pub(crate) use edit_requirement::{infer_task_edit_requirement, resolve_task_edit_requirement};
pub(crate) use inline_review_context::prepend_review_context_to_prompt;
pub(crate) use prompt::{read_prompt, resolve_positional_stdin_sentinel};
pub(crate) use routing_conflict::{is_routing_conflict, routing_conflict_error};
pub(crate) use tool_availability::{
    ToolBinaryAvailability, is_tool_binary_available_for_config, resolved_claude_code_transport,
    resolved_codex_transport, resolved_tool_binary_name, tool_binary_availability,
};

#[cfg(test)]
pub(crate) const TEST_SKIP_TOOL_AVAILABILITY_CHECK_ENV: &str =
    "CSA_TEST_SKIP_TOOL_AVAILABILITY_CHECK";

#[cfg(test)]
pub(crate) const TEST_ASSUME_TOOLS_AVAILABLE_ENV: &str = "CSA_TEST_ASSUME_TOOLS_AVAILABLE";

/// Reject the contradictory routing combination where a direct tool request
/// also asks both to use and ignore tier routing.
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

/// Resolve tool and model from CLI args and config.
///
/// Returns (tool, model_spec, model) where:
/// - tool: the selected tool (from CLI or tier-based selection)
/// - model_spec: optional model spec string (from CLI or tier)
/// - model: optional model string (from CLI, with alias resolution applied)
///
/// When tool is None, uses tier-based round-robin selection.
/// `needs_edit`: when true, filters out tools with `allow_edit_existing_files = false`.
/// `tool_is_auto_resolved`: when true, the `tool` param was auto-selected (not user CLI),
///   so it should not trigger tier enforcement blocking.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_tool_and_model(
    tool: Option<ToolName>,
    model_spec: Option<&str>,
    model: Option<&str>,
    config: Option<&ProjectConfig>,
    project_root: &Path,
    force: bool,
    force_override_user_config: bool,
    needs_edit: bool,
    tier: Option<&str>,
    force_ignore_tier_setting: bool,
    tool_is_auto_resolved: bool,
) -> Result<(ToolName, Option<String>, Option<String>)> {
    let tiers_configured = config.is_some_and(|c| !c.tiers.is_empty());
    let bypass_tier = force_ignore_tier_setting || force_override_user_config;
    let exact_selection_active = model_spec.is_some();

    // Enforce tier routing: block direct --tool/--model/--thinking when tiers are configured,
    // unless --force-ignore-tier-setting (or --force) is active. --model-spec is the
    // exact-selection escape hatch and is handled below.
    // Auto-resolved tools (from HeterogeneousPreferred etc.) don't count as user-explicit.
    let tool_triggers_enforcement = tool.is_some() && !tool_is_auto_resolved;
    validate_tool_tier_override_flags(tool_triggers_enforcement, tier, force_ignore_tier_setting)?;
    if tiers_configured
        && !bypass_tier
        && tier.is_none()
        && !exact_selection_active
        && (tool_triggers_enforcement || model.is_some())
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
             or add --force-ignore-tier-setting to override.\n\
             Available tiers: [{tier_list}]{alias_hint}\n\
             Hint: omit --tool entirely to use auto-selection, or use --tool auto"
        );
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

    // Case 0: --tier provided → resolve tool/model from tier definition.
    // A user-explicit `--tool` acts as a filter inside the selected tier.
    if let Some(ref canonical_name) = canonical_tier
        && let Some(cfg) = config
    {
        let resolution = if let Some(requested_tool) = tool.filter(|_| !tool_is_auto_resolved) {
            resolve_requested_tool_from_tier(
                canonical_name,
                cfg,
                None,
                requested_tool,
                force_override_user_config,
                &[],
            )?
        } else if let Some(resolution) =
            resolve_tool_from_tier(canonical_name, cfg, None, None, &[])
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

    // Case 1: model_spec provided → parse it to get tool. --model-spec is the
    // exact-selection escape hatch and implicitly bypasses tier whitelist;
    // enforce_tool_enabled still applies.
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
        return Ok((tool_name, None, resolved_model));
    }

    // Case 3: neither tool nor model_spec → use round-robin tier-based selection.
    // When --force is active, bypass tiers and pick any installed+enabled tool.
    if force {
        for tool in csa_config::global::all_known_tools() {
            let name = tool.as_str();
            let enabled = config.is_none_or(|cfg| cfg.is_tool_enabled(name));
            if enabled && is_tool_binary_available_for_config(name, config) {
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
        // Try round-robin rotation first (needs project root to persist state)
        if let Ok(Some((tool_name_str, tier_model_spec))) =
            csa_scheduler::resolve_tier_tool_rotated(cfg, "default", project_root, needs_edit)
        {
            let tool_name = parse_tool_name(&tool_name_str)?;
            return Ok((tool_name, Some(tier_model_spec), resolved_model));
        }
        // Fallback: original non-rotating selection (also respects edit restrictions)
        if let Some((tool_name_str, tier_model_spec)) =
            cfg.resolve_tier_tool_filtered("default", needs_edit)
        {
            let tool_name = parse_tool_name(&tool_name_str)?;
            return Ok((tool_name, Some(tier_model_spec), resolved_model));
        }
    }

    // Fallback: minimal-init configs with empty tiers — pick any auto-selectable installed tool.
    // Only activates when tiers are empty to avoid silently bypassing configured tier mappings.
    if let Some(cfg) = config
        && cfg.tiers.is_empty()
    {
        for tool in csa_config::global::all_known_tools() {
            let name = tool.as_str();
            if cfg.is_tool_auto_selectable(name)
                && is_tool_binary_available_for_config(name, Some(cfg))
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

        // Smart-parse --model: split trailing thinking suffix (e.g. "/xhigh")
        // when --thinking is not explicitly provided.
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

        // Explicit --thinking > thinking parsed from --model suffix > config default
        let effective_thinking = thinking.or_else(|| {
            apply_tool_defaults
                .then(|| config.and_then(|cfg| cfg.tool_default_thinking(tool_name)))?
        });
        let budget = if let Some(t) = effective_thinking {
            Some(ThinkingBudget::parse(t)?)
        } else {
            model_thinking
        };

        Executor::from_tool_name(tool, final_model, budget)
    };

    // When model_spec is present, the model and thinking come from the spec.
    // Explicit arguments must override them (CLI/config > tier spec).
    if model_spec.is_some() {
        if let Some(explicit_model) = model {
            // Also smart-parse model override for thinking suffix.
            let (clean, suffix_budget) = ThinkingBudget::try_split_from_model(explicit_model);
            executor.override_model(clean.to_string());
            // Explicit --thinking takes precedence over suffix in override too.
            if thinking.is_none()
                && let Some(budget) = suffix_budget
            {
                executor.override_thinking_budget(budget);
            }
        }
        if let Some(explicit_thinking) = thinking {
            let budget = ThinkingBudget::parse(explicit_thinking)?;
            executor.override_thinking_budget(budget);
        }
    }

    if matches!(executor, Executor::Codex { .. }) {
        let transport = tool_availability::resolved_codex_transport(config);
        executor.override_codex_transport(transport);
    }
    if matches!(executor, Executor::ClaudeCode { .. }) {
        let transport = tool_availability::resolved_claude_code_transport(config);
        executor.override_claude_code_transport(transport);
    }

    Ok(executor)
}

pub(crate) fn model_name_for_tier_validation(model: Option<&str>) -> Option<&str> {
    model.map(|name| ThinkingBudget::try_split_from_model(name).0)
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

/// Parse token usage from tool output (best-effort, returns None on failure).
///
/// Looks for common patterns in stdout/stderr:
/// - "tokens: N" or "Tokens: N" or "total_tokens: N"
/// - "input_tokens: N" / "output_tokens: N"
/// - "cost: $N.NN" or "estimated_cost: $N.NN"
pub(crate) fn parse_token_usage(output: &str) -> Option<TokenUsage> {
    let mut usage = TokenUsage::default();
    let mut found_any = false;

    // Simple pattern matching without regex
    for line in output.lines() {
        let line_lower = line.to_lowercase();

        // Parse input_tokens
        if let Some(pos) = line_lower.find("input_tokens")
            && let Some(val) = extract_number(&line[pos..])
        {
            usage.input_tokens = Some(val);
            found_any = true;
        }

        // Parse output_tokens
        if let Some(pos) = line_lower.find("output_tokens")
            && let Some(val) = extract_number(&line[pos..])
        {
            usage.output_tokens = Some(val);
            found_any = true;
        }

        // Parse total_tokens
        if let Some(pos) = line_lower.find("total_tokens") {
            if let Some(val) = extract_number(&line[pos..]) {
                usage.total_tokens = Some(val);
                found_any = true;
            }
        } else if let Some(pos) = line_lower.find("tokens:") {
            // Only match standalone "tokens:" — skip if preceded by a letter or
            // underscore (e.g. "input_tokens:" or "output_tokens:" already
            // handled above).
            let prev = line_lower.as_bytes().get(pos.wrapping_sub(1)).copied();
            let is_standalone = pos == 0 || !matches!(prev, Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'));
            if is_standalone && let Some(val) = extract_number(&line[pos..]) {
                usage.total_tokens = Some(val);
                found_any = true;
            }
        }

        // Parse cost (look for "$N.NN" pattern)
        if line_lower.contains("cost")
            && let Some(val) = extract_cost(line)
        {
            usage.estimated_cost_usd = Some(val);
            found_any = true;
        }
    }

    // Calculate total_tokens if not found but input/output are available
    if usage.total_tokens.is_none()
        && let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens)
    {
        usage.total_tokens = Some(input + output);
        found_any = true;
    }

    if found_any { Some(usage) } else { None }
}

/// Extract a number after colon or equals sign.
fn extract_number(text: &str) -> Option<u64> {
    // Find colon or equals
    let start = text.find(':')?;
    let after_colon = &text[start + 1..];

    // Take first word after colon
    let num_str: String = after_colon
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();

    num_str.parse().ok()
}

/// Extract cost value after $ sign.
fn extract_cost(text: &str) -> Option<f64> {
    let start = text.find('$')?;
    let after_dollar = &text[start + 1..];

    // Take digits and decimal point
    let num_str: String = after_dollar
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();

    num_str.parse().ok()
}

/// Resolve prompt from `--prompt-file`, positional arg, or stdin (in priority order).
pub(crate) fn resolve_prompt_with_file(
    prompt: Option<String>,
    prompt_file: Option<&std::path::Path>,
) -> Result<String> {
    if let Some(path) = prompt_file {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("--prompt-file: failed to read '{}'", path.display()))?;
        if content.trim().is_empty() {
            anyhow::bail!("--prompt-file '{}' is empty", path.display());
        }
        return Ok(content);
    }
    read_prompt(prompt)
}

/// Result of resolving a tool from a tier's models list.
#[derive(Debug, Clone)]
pub(crate) struct TierToolResolution {
    /// The resolved tool name.
    pub tool: ToolName,
    /// The full model spec string (e.g., "gemini-cli/google/gemini-3.1-pro-preview/xhigh").
    pub model_spec: String,
}

/// Collect all enabled + available model specs from a named tier in config order.
///
/// Applies the same availability and whitelist rules as tier resolution so
/// callers can consistently build reviewer pools or per-tool model-spec maps.
pub(crate) fn collect_available_tier_models(
    tier_name: &str,
    config: &ProjectConfig,
    whitelist: Option<&[String]>,
    skip_specs: &[String],
) -> Vec<TierToolResolution> {
    let Some(tier) = config.tiers.get(tier_name) else {
        return Vec::new();
    };

    tier.models
        .iter()
        .filter_map(|spec| {
            if skip_specs.iter().any(|s| s == spec) {
                return None;
            }
            let parts: Vec<&str> = spec.splitn(4, '/').collect();
            if parts.len() != 4 {
                return None;
            }
            let tool_str = parts[0];
            let tool = parse_tool_name(tool_str).ok()?;
            if !config.is_tool_enabled(tool_str)
                || !is_tool_binary_available_for_config(tool_str, Some(config))
            {
                return None;
            }
            if let Some(wl) = whitelist
                && !wl.iter().any(|w| w == tool_str)
            {
                return None;
            }
            Some(TierToolResolution {
                tool,
                model_spec: spec.clone(),
            })
        })
        .collect()
}

/// Resolve a user-requested tool from a tier, preserving tier ordering while
/// failing clearly when the tool is absent from that tier.
pub(crate) fn resolve_requested_tool_from_tier(
    tier_name: &str,
    config: &ProjectConfig,
    parent_tool: Option<&str>,
    requested_tool: ToolName,
    force_override_user_config: bool,
    skip_specs: &[String],
) -> Result<TierToolResolution> {
    let requested_tool_name = requested_tool.as_str();
    let Some(tier) = config.tiers.get(tier_name) else {
        anyhow::bail!("Tier '{}' not found.", tier_name);
    };
    let tool_in_tier = tier.models.iter().any(|spec| {
        !skip_specs.iter().any(|skip| skip == spec)
            && spec
                .split('/')
                .next()
                .is_some_and(|tool_name| tool_name == requested_tool_name)
    });
    if !tool_in_tier {
        let suggestions = config.suggest_compatible_alternatives(requested_tool_name, tier_name);
        anyhow::bail!(
            "Tool '{}' is not available in tier '{}'\n\n{}",
            requested_tool_name,
            tier_name,
            suggestions
        );
    }

    config.enforce_tool_enabled(requested_tool_name, force_override_user_config)?;

    let whitelist = [requested_tool_name.to_string()];
    if let Some(resolution) =
        resolve_tool_from_tier(tier_name, config, parent_tool, Some(&whitelist), skip_specs)
    {
        return Ok(resolution);
    }

    anyhow::bail!(
        "Requested tool '{}' is configured in tier '{}' but is not currently available. \
         Ensure it is installed and enabled.",
        requested_tool_name,
        tier_name
    );
}

/// Resolve a tool from a named tier's models list with heterogeneous preference.
///
/// Filters tier models by enabled + binary available, then prefers a tool from
/// a different `ModelFamily` than `parent_tool`. Falls back to any available tool
/// in the tier when no heterogeneous option exists.
///
/// `skip_specs` excludes model specs that have already been tried (e.g. due to
/// 429 rate-limit failover within the same tier).
///
/// **Whitelist interaction (#648)**: When `whitelist` is `Some`, only tier models
/// whose tool name appears in the whitelist are considered. This is how
/// `[review].tool` and `[debate].tool` narrow a multi-tool tier to a single tool.
/// Pass `None` to use the tier's full fallback chain.
///
/// Returns `None` if no enabled, available tool is found in the tier.
pub(crate) fn resolve_tool_from_tier(
    tier_name: &str,
    config: &ProjectConfig,
    parent_tool: Option<&str>,
    whitelist: Option<&[String]>,
    skip_specs: &[String],
) -> Option<TierToolResolution> {
    let parent_family = parent_tool
        .and_then(|p| parse_tool_name(p).ok())
        .map(|t| t.model_family());

    let available = collect_available_tier_models(tier_name, config, whitelist, skip_specs);

    if available.is_empty() {
        return None;
    }

    // Prefer heterogeneous (different model family from parent)
    if let Some(parent_fam) = parent_family
        && let Some(resolution) = available
            .iter()
            .find(|resolution| resolution.tool.model_family() != parent_fam)
    {
        return Some(resolution.clone());
    }

    // No heterogeneous option (or no parent) — use first available
    Some(available[0].clone())
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
pub(crate) fn resolve_tool(detected: Option<String>, config: &GlobalConfig) -> Option<String> {
    detected.or_else(|| config.defaults.tool.clone())
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
#[path = "run_helpers_transport_tests.rs"]
mod transport_tests;

#[cfg(test)]
#[path = "run_helpers_model_spec_tests.rs"]
mod model_spec_tests;

#[cfg(test)]
#[path = "run_helpers_override_tests.rs"]
mod override_tests;

#[cfg(test)]
#[path = "run_helpers_inline_review_context_tests.rs"]
mod inline_review_context_tests;

#[cfg(test)]
#[path = "run_helpers_transport_integration_tests.rs"]
mod transport_integration_tests;
