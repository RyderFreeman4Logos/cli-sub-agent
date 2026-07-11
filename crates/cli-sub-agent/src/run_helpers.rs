//! Helper functions for `csa run`: tool resolution, executor building, token parsing.

use anyhow::Result;

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_executor::ModelSpec;

#[path = "run_helpers_atomic_commit.rs"]
mod atomic_commit;
#[path = "run_helpers_basics.rs"]
mod basics;
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
pub(crate) use basics::{
    detect_parent_tool, is_compress_command, parse_tool_name, resolve_tool, truncate_prompt,
};
pub(crate) use compound_tier::{
    apply_compound_tier_selector, apply_compound_tier_selector_arg, compound_tier_selects_tool,
};
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
    TierToolResolution, collect_available_tier_models_with_catalog,
    collect_preferred_tier_models_with_catalog, evaluate_tier_models_with_catalog,
    resolve_preferred_tool_from_tier_with_catalog,
    resolve_runtime_available_tier_fallback_with_catalog, resolve_tool_from_tier_with_catalog,
    validate_tier_model_spec_compatibility_with_catalog,
};
#[cfg(test)]
pub(crate) use tier_resolution::{
    collect_preferred_tier_models, resolve_preferred_tool_from_tier, resolve_tool_from_tier,
    resolve_tool_from_tier_with_global_config,
};
pub(crate) use token_parse::parse_token_usage;
#[cfg(test)]
pub(crate) use token_parse::{extract_cost, extract_number};
pub(crate) use tool_availability::{
    ToolBinaryAvailability, is_tool_binary_available_for_config,
    is_tool_runtime_available_for_config_with_env, resolved_claude_code_transport,
    resolved_codex_transport, resolved_tool_binary_name, tool_binary_availability,
    tool_runtime_availability_with_env,
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

    let mut available: Vec<&str> = cfg.tiers.keys().map(|k| k.as_str()).collect();
    available.sort_unstable();
    let alias_hint = cfg.format_tier_aliases();
    anyhow::bail!(
        "Direct --tool is restricted when tiers are configured. \
         Use --tier <name> to specify which tier's model/thinking config to use, \
         or use --tier <name> --tool <tool> to prefer a tool inside that tier. \
         Use --hint-difficulty <label> to route through [tier_mapping]. \
         Emergency direct-tool bypasses require \
         [tier_policy].allow_force_bypass = true in the global CSA config. \
         Available tiers: [{}]{alias_hint}",
        available.join(", ")
    );
}

pub(crate) fn format_run_direct_tool_tier_policy_error(cfg: &ProjectConfig) -> String {
    let mut available: Vec<&str> = cfg.tiers.keys().map(|k| k.as_str()).collect();
    available.sort_unstable();
    let alias_hint = cfg.format_tier_aliases();
    format!(
        "Direct --tool is blocked when tiers are configured.\n\
         Use --tier <name> for tier-based routing. To prefer a tool inside that tier, \
         use --tier <name> --tool <tool>.\n\
         Use --auto-route <intent> or --hint-difficulty <label> to route through \
         [tier_mapping]. Emergency exact-model/force bypasses require \
         [tier_policy].allow_force_bypass = true in the global CSA config.\n\
         Example: csa run --sa-mode <true|false> --tier <name> --tool <tool> ...\n\
         Available tiers: [{}]{alias_hint}",
        available.join(", ")
    )
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
/// (e.g., an explicit-only or read-only tool for write tasks).
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
             specify --tool auto|claude-code|codex|opencode to control tool selection"
        );
        eprintln!(
            "warning: --tier without --tool uses auto-routing; \
             specify --tool auto|claude-code|codex|opencode to control tool selection"
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
#[path = "run_helpers_resolve.rs"]
mod resolve;
pub(crate) use resolve::resolve_tool_and_model;

#[cfg(test)]
#[path = "run_helpers_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "run_helpers_tests_prompt.rs"]
mod tests_prompt;

#[cfg(test)]
#[path = "run_helpers_token_usage_tests.rs"]
mod token_usage_tests;

#[cfg(test)]
#[path = "run_helpers_tests_tail.rs"]
mod tests_tail;

#[cfg(test)]
#[path = "run_helpers_tier_tests.rs"]
mod tier_tests;

#[cfg(test)]
#[path = "run_helpers_catalog_tests.rs"]
mod catalog_tests;

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
