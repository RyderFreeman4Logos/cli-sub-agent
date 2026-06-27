//! Tests for compound tier-tool selector wiring (#1441).
//!
//! Unit coverage of `apply_compound_tier_selector` and
//! `apply_compound_tier_selector_arg` — verifies that compound `--tier
//! <tier>-<tool>` parsing fires only when the literal tier is unknown, returns
//! the canonical tier + parsed tool when successful, and surfaces a routing
//! error on tool conflict.

use super::*;
use crate::run_helpers::is_routing_conflict;
use csa_config::config::CURRENT_SCHEMA_VERSION;
use csa_config::{ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, TierStrategy};
use csa_core::types::{ToolArg, ToolName};
use std::collections::HashMap;

fn fixture(tool_aliases: HashMap<String, String>) -> ProjectConfig {
    let mut tiers = HashMap::new();
    for name in ["tier-3-complex", "tier-4-critical"] {
        tiers.insert(
            name.to_string(),
            TierConfig {
                description: "test".to_string(),
                models: vec!["codex/openai/gpt-5.4/high".to_string()],
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        );
    }
    ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases,
        preferences: None,
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

// --- apply_compound_tier_selector (Option<ToolName>) -----------------------

#[test]
fn compound_selector_parses_canonical_tool_suffix() {
    let cfg = fixture(HashMap::new());
    let (tier, tool) =
        apply_compound_tier_selector(Some("tier-4-critical-codex".to_string()), None, Some(&cfg))
            .expect("compound parse should succeed");
    assert_eq!(tier.as_deref(), Some("tier-4-critical"));
    assert_eq!(tool, Some(ToolName::Codex));
}

#[test]
fn compound_selector_parses_multi_hyphen_tool_suffix() {
    let cfg = fixture(HashMap::new());
    let (tier, tool) = apply_compound_tier_selector(
        Some("tier-4-critical-claude-code".to_string()),
        None,
        Some(&cfg),
    )
    .expect("compound parse should succeed");
    assert_eq!(tier.as_deref(), Some("tier-4-critical"));
    assert_eq!(tool, Some(ToolName::ClaudeCode));
}

#[test]
fn compound_selector_parses_builtin_alias_suffix() {
    let cfg = fixture(HashMap::new());
    let (tier, tool) =
        apply_compound_tier_selector(Some("tier-4-critical-claude".to_string()), None, Some(&cfg))
            .expect("compound parse should succeed");
    assert_eq!(tier.as_deref(), Some("tier-4-critical"));
    assert_eq!(tool, Some(ToolName::ClaudeCode));
}

#[test]
fn compound_selector_leaves_known_tier_untouched() {
    let cfg = fixture(HashMap::new());
    let (tier, tool) =
        apply_compound_tier_selector(Some("tier-4-critical".to_string()), None, Some(&cfg))
            .expect("direct tier should not enter compound parsing");
    assert_eq!(tier.as_deref(), Some("tier-4-critical"));
    assert_eq!(tool, None);
}

#[test]
fn compound_selector_leaves_unknown_compound_untouched() {
    let cfg = fixture(HashMap::new());
    let (tier, tool) =
        apply_compound_tier_selector(Some("nonexistent-codex".to_string()), None, Some(&cfg))
            .expect("non-matching compound should return tier verbatim for downstream handling");
    assert_eq!(tier.as_deref(), Some("nonexistent-codex"));
    assert_eq!(tool, None);
}

#[test]
fn compound_selector_errors_on_tool_conflict() {
    let cfg = fixture(HashMap::new());
    let err = apply_compound_tier_selector(
        Some("tier-4-critical-codex".to_string()),
        Some(ToolName::Opencode),
        Some(&cfg),
    )
    .expect_err("conflicting --tool should error");
    assert!(is_routing_conflict(&err));
    let msg = format!("{err:#}");
    assert!(msg.contains("tier-4-critical-codex"), "msg: {msg}");
    assert!(msg.contains("codex"), "msg: {msg}");
    assert!(msg.contains("opencode"), "msg: {msg}");
}

#[test]
fn compound_selector_passes_through_when_tool_matches() {
    let cfg = fixture(HashMap::new());
    let (tier, tool) = apply_compound_tier_selector(
        Some("tier-4-critical-codex".to_string()),
        Some(ToolName::Codex),
        Some(&cfg),
    )
    .expect("matching --tool should be accepted");
    assert_eq!(tier.as_deref(), Some("tier-4-critical"));
    assert_eq!(tool, Some(ToolName::Codex));
}

#[test]
fn compound_selector_returns_none_tier_when_none() {
    let cfg = fixture(HashMap::new());
    let (tier, tool) =
        apply_compound_tier_selector(None, Some(ToolName::Codex), Some(&cfg)).unwrap();
    assert_eq!(tier, None);
    assert_eq!(tool, Some(ToolName::Codex));
}

#[test]
fn compound_selector_passes_through_when_no_config() {
    let (tier, tool) =
        apply_compound_tier_selector(Some("tier-4-critical-codex".to_string()), None, None)
            .expect("no config means no compound parsing");
    assert_eq!(tier.as_deref(), Some("tier-4-critical-codex"));
    assert_eq!(tool, None);
}

// --- apply_compound_tier_selector_arg (Option<ToolArg>) --------------------

#[test]
fn compound_selector_arg_injects_tool_when_none() {
    let cfg = fixture(HashMap::new());
    let (tier, tool) = apply_compound_tier_selector_arg(
        Some("tier-4-critical-codex".to_string()),
        None,
        Some(&cfg),
    )
    .expect("compound should inject tool");
    assert_eq!(tier.as_deref(), Some("tier-4-critical"));
    assert!(matches!(tool, Some(ToolArg::Specific(ToolName::Codex))));
}

#[test]
fn compound_selector_arg_overrides_auto() {
    let cfg = fixture(HashMap::new());
    let (tier, tool) = apply_compound_tier_selector_arg(
        Some("tier-4-critical-codex".to_string()),
        Some(ToolArg::Auto),
        Some(&cfg),
    )
    .expect("Auto should be replaced by compound");
    assert_eq!(tier.as_deref(), Some("tier-4-critical"));
    assert!(matches!(tool, Some(ToolArg::Specific(ToolName::Codex))));
}

#[test]
fn compound_selector_arg_overrides_any_available() {
    let cfg = fixture(HashMap::new());
    let (tier, tool) = apply_compound_tier_selector_arg(
        Some("tier-4-critical-codex".to_string()),
        Some(ToolArg::AnyAvailable),
        Some(&cfg),
    )
    .expect("AnyAvailable should be replaced by compound");
    assert_eq!(tier.as_deref(), Some("tier-4-critical"));
    assert!(matches!(tool, Some(ToolArg::Specific(ToolName::Codex))));
}

#[test]
fn compound_selector_arg_errors_on_specific_conflict() {
    let cfg = fixture(HashMap::new());
    let err = apply_compound_tier_selector_arg(
        Some("tier-4-critical-codex".to_string()),
        Some(ToolArg::Specific(ToolName::Opencode)),
        Some(&cfg),
    )
    .expect_err("Specific(other) should error");
    assert!(is_routing_conflict(&err));
}

#[test]
fn compound_selector_arg_passes_specific_match() {
    let cfg = fixture(HashMap::new());
    let (tier, tool) = apply_compound_tier_selector_arg(
        Some("tier-4-critical-codex".to_string()),
        Some(ToolArg::Specific(ToolName::Codex)),
        Some(&cfg),
    )
    .expect("Specific match should pass through");
    assert_eq!(tier.as_deref(), Some("tier-4-critical"));
    assert!(matches!(tool, Some(ToolArg::Specific(ToolName::Codex))));
}

#[test]
fn compound_selector_arg_resolves_user_alias_match() {
    let mut tool_aliases = HashMap::new();
    tool_aliases.insert("cx".to_string(), "codex".to_string());
    let cfg = fixture(tool_aliases);
    let (tier, tool) = apply_compound_tier_selector_arg(
        Some("tier-4-critical-codex".to_string()),
        Some(ToolArg::Alias("cx".to_string())),
        Some(&cfg),
    )
    .expect("user alias resolving to compound tool should match");
    assert_eq!(tier.as_deref(), Some("tier-4-critical"));
    assert!(matches!(tool, Some(ToolArg::Specific(ToolName::Codex))));
}

#[test]
fn compound_selector_arg_unresolvable_alias_defers_to_downstream() {
    let cfg = fixture(HashMap::new());
    let (tier, tool) = apply_compound_tier_selector_arg(
        Some("tier-4-critical-codex".to_string()),
        Some(ToolArg::Alias("unknown-alias".to_string())),
        Some(&cfg),
    )
    .expect("unresolvable alias should not block compound — compound's tool wins");
    assert_eq!(tier.as_deref(), Some("tier-4-critical"));
    assert!(matches!(tool, Some(ToolArg::Specific(ToolName::Codex))));
}

#[test]
fn compound_selector_arg_leaves_known_tier_untouched() {
    let cfg = fixture(HashMap::new());
    let (tier, tool) = apply_compound_tier_selector_arg(
        Some("tier-4-critical".to_string()),
        Some(ToolArg::Auto),
        Some(&cfg),
    )
    .expect("direct tier should bypass compound parsing");
    assert_eq!(tier.as_deref(), Some("tier-4-critical"));
    assert!(matches!(tool, Some(ToolArg::Auto)));
}
