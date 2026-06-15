use super::routing::resolve_run_no_failover;
use crate::run_cmd_model_pin::{
    explicit_tool_no_failover_from_inherited_pin, validate_inherited_model_pin_allows_explicit_tool,
};
use csa_core::types::{ToolArg, ToolName, ToolSelectionStrategy};

#[test]
fn run_explicit_tool_with_tier_allow_fallback_keeps_failover_enabled() {
    assert!(!resolve_run_no_failover(
        true,
        true,
        &ToolSelectionStrategy::Explicit(ToolName::Codex),
        false,
        true,
    ));
}

#[test]
fn inherited_gemini_pin_rejects_explicit_codex_tool() {
    let err = validate_inherited_model_pin_allows_explicit_tool(
        Some(&ToolArg::Specific(ToolName::Codex)),
        true,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
    )
    .expect_err("explicit codex must not inherit a gemini model pin");
    let msg = err.to_string();

    assert!(msg.contains("explicit --tool codex"), "{msg}");
    assert!(msg.contains("CSA_MODEL_SPEC"), "{msg}");
    assert!(msg.contains("gemini-cli"), "{msg}");
    assert!(
        msg.contains("refusing to route the explicit codex request through gemini-cli"),
        "{msg}"
    );
}

#[test]
fn inherited_codex_pin_allows_explicit_codex_tool() {
    validate_inherited_model_pin_allows_explicit_tool(
        Some(&ToolArg::Specific(ToolName::Codex)),
        true,
        Some("codex/openai/gpt-5.5/xhigh"),
    )
    .expect("matching inherited codex pin should be allowed");
}

#[test]
fn inherited_pin_keeps_explicit_codex_no_failover_constraint() {
    assert!(explicit_tool_no_failover_from_inherited_pin(
        Some(&ToolArg::Specific(ToolName::Codex)),
        true,
        false,
    ));
    assert!(!explicit_tool_no_failover_from_inherited_pin(
        Some(&ToolArg::Specific(ToolName::Codex)),
        true,
        true,
    ));
    assert!(!explicit_tool_no_failover_from_inherited_pin(
        Some(&ToolArg::Auto),
        true,
        false,
    ));
}
