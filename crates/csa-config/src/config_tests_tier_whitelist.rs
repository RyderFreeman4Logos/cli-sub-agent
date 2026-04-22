use super::*;
use crate::config::TierStrategy;

fn config_with_tiers(tier_models: &[&str]) -> ProjectConfig {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-2-standard".to_string(),
        TierConfig {
            description: "test tier".to_string(),
            models: tier_models.iter().map(|s| s.to_string()).collect(),
            strategy: TierStrategy::default(),

            token_budget: None,
            max_turns: None,
        },
    );
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
        tool_aliases: HashMap::new(),
        preferences: None,
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

#[test]
fn is_model_spec_in_tiers_exact_match() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    assert!(cfg.is_model_spec_in_tiers("codex/openai/gpt-5.3-codex/high"));
}

#[test]
fn is_model_spec_in_tiers_no_match() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    assert!(!cfg.is_model_spec_in_tiers("codex/openai/gpt-4o/high"));
}

#[test]
fn is_model_spec_in_tiers_empty_tiers() {
    let cfg = ProjectConfig {
        tiers: HashMap::new(),
        ..config_with_tiers(&[])
    };
    assert!(!cfg.is_model_spec_in_tiers("codex/openai/gpt-5.3-codex/high"));
}

#[test]
fn allowed_model_specs_for_tool_filters_correctly() {
    let cfg = config_with_tiers(&[
        "codex/openai/gpt-5.3-codex/high",
        "claude-code/anthropic/sonnet-4.5/xhigh",
        "codex/openai/gpt-5.3-codex-spark/low",
    ]);
    let allowed = cfg.allowed_model_specs_for_tool("codex");
    assert_eq!(allowed.len(), 2);
    assert!(allowed.contains(&"codex/openai/gpt-5.3-codex/high".to_string()));
    assert!(allowed.contains(&"codex/openai/gpt-5.3-codex-spark/low".to_string()));
}

#[test]
fn enforce_tier_whitelist_empty_tiers_allows_all() {
    let cfg = ProjectConfig {
        tiers: HashMap::new(),
        ..config_with_tiers(&[])
    };
    assert!(cfg.enforce_tier_whitelist("codex", None).is_ok());
    assert!(
        cfg.enforce_tier_whitelist("codex", Some("codex/openai/gpt-4o/high"))
            .is_ok()
    );
}

#[test]
fn enforce_tier_whitelist_tool_in_tiers_ok() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    assert!(cfg.enforce_tier_whitelist("codex", None).is_ok());
}

#[test]
fn enforce_tier_whitelist_tool_not_in_tiers_rejected() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    let err = cfg.enforce_tier_whitelist("opencode", None).unwrap_err();
    assert!(err.to_string().contains("not configured in any tier"));
    assert!(err.to_string().contains("opencode"));
}

#[test]
fn enforce_tier_whitelist_model_spec_in_tiers_ok() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    assert!(
        cfg.enforce_tier_whitelist("codex", Some("codex/openai/gpt-5.3-codex/high"))
            .is_ok()
    );
}

#[test]
fn enforce_tier_whitelist_model_spec_not_in_tiers_rejected() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    let err = cfg
        .enforce_tier_whitelist("codex", Some("codex/openai/gpt-4o/high"))
        .unwrap_err();
    assert!(err.to_string().contains("not configured in any tier"));
    assert!(err.to_string().contains("gpt-4o"));
}

#[test]
fn enforce_tier_whitelist_tool_ok_but_wrong_spec_rejected() {
    let cfg = config_with_tiers(&[
        "codex/openai/gpt-5.3-codex/high",
        "claude-code/anthropic/sonnet-4.5/xhigh",
    ]);
    // Tool exists in tiers, but this specific spec doesn't
    let err = cfg
        .enforce_tier_whitelist("codex", Some("codex/openai/gpt-3.5-turbo/low"))
        .unwrap_err();
    assert!(err.to_string().contains("not configured in any tier"));
    assert!(err.to_string().contains("Allowed specs for 'codex'"));
}

#[test]
fn is_model_name_in_tiers_for_tool_exact_match() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    assert!(cfg.is_model_name_in_tiers_for_tool("codex", "gpt-5.3-codex"));
}

#[test]
fn is_model_name_in_tiers_for_tool_no_match() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    assert!(!cfg.is_model_name_in_tiers_for_tool("codex", "gpt-4o"));
}

#[test]
fn is_model_name_in_tiers_for_tool_wrong_tool() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    assert!(!cfg.is_model_name_in_tiers_for_tool("claude-code", "gpt-5.3-codex"));
}

#[test]
fn enforce_tier_model_name_empty_tiers_allows_all() {
    let cfg = ProjectConfig {
        tiers: HashMap::new(),
        ..config_with_tiers(&[])
    };
    assert!(cfg.enforce_tier_model_name("codex", Some("gpt-4o")).is_ok());
}

#[test]
fn enforce_tier_model_name_none_model_allows() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    assert!(cfg.enforce_tier_model_name("codex", None).is_ok());
}

#[test]
fn enforce_tier_model_name_configured_model_ok() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    assert!(
        cfg.enforce_tier_model_name("codex", Some("gpt-5.3-codex"))
            .is_ok()
    );
}

#[test]
fn enforce_tier_model_name_unconfigured_model_rejected() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    let err = cfg
        .enforce_tier_model_name("codex", Some("gpt-4o"))
        .unwrap_err();
    assert!(err.to_string().contains("not configured in any tier"));
    assert!(err.to_string().contains("gpt-4o"));
    assert!(err.to_string().contains("Allowed models for 'codex'"));
}

#[test]
fn enforce_tier_model_name_full_spec_delegates_to_spec_check() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    // Alias-resolved full spec should be accepted via spec-level check
    assert!(
        cfg.enforce_tier_model_name("codex", Some("codex/openai/gpt-5.3-codex/high"))
            .is_ok()
    );
}

#[test]
fn enforce_tier_model_name_full_spec_unconfigured_rejected() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    let err = cfg
        .enforce_tier_model_name("codex", Some("codex/openai/gpt-4o/high"))
        .unwrap_err();
    assert!(err.to_string().contains("not configured in any tier"));
}

#[test]
fn enforce_tier_whitelist_cross_tool_spec_rejected() {
    let cfg = config_with_tiers(&[
        "codex/openai/gpt-5.3-codex/high",
        "claude-code/anthropic/sonnet-4.5/xhigh",
    ]);
    // Spec belongs to claude-code, but tool is codex — must reject
    let err = cfg
        .enforce_tier_whitelist("codex", Some("claude-code/anthropic/sonnet-4.5/xhigh"))
        .unwrap_err();
    assert!(err.to_string().contains("belongs to tool"));
    assert!(err.to_string().contains("claude-code"));
}

#[test]
fn enforce_tier_model_name_cross_tool_full_spec_rejected() {
    let cfg = config_with_tiers(&[
        "codex/openai/gpt-5.3-codex/high",
        "claude-code/anthropic/sonnet-4.5/xhigh",
    ]);
    // Full spec for claude-code passed with tool=codex — must reject
    let err = cfg
        .enforce_tier_model_name("codex", Some("claude-code/anthropic/sonnet-4.5/xhigh"))
        .unwrap_err();
    assert!(err.to_string().contains("belongs to tool"));
}

#[test]
fn is_model_name_in_tiers_for_tool_provider_model_format() {
    let cfg = config_with_tiers(&["opencode/google/gemini-2.5-pro/medium"]);
    // Provider/model format should match
    assert!(cfg.is_model_name_in_tiers_for_tool("opencode", "google/gemini-2.5-pro"));
    // Wrong provider should not match
    assert!(!cfg.is_model_name_in_tiers_for_tool("opencode", "anthropic/gemini-2.5-pro"));
    // Wrong tool should not match
    assert!(!cfg.is_model_name_in_tiers_for_tool("codex", "google/gemini-2.5-pro"));
    // Bare model name should still match
    assert!(cfg.is_model_name_in_tiers_for_tool("opencode", "gemini-2.5-pro"));
}

#[test]
fn enforce_tier_model_name_provider_model_format_ok() {
    let cfg = config_with_tiers(&["opencode/google/gemini-2.5-pro/medium"]);
    assert!(
        cfg.enforce_tier_model_name("opencode", Some("google/gemini-2.5-pro"))
            .is_ok()
    );
}

#[test]
fn enforce_tier_model_name_provider_model_format_wrong_provider_rejected() {
    let cfg = config_with_tiers(&["opencode/google/gemini-2.5-pro/medium"]);
    let err = cfg
        .enforce_tier_model_name("opencode", Some("anthropic/gemini-2.5-pro"))
        .unwrap_err();
    assert!(err.to_string().contains("not configured in any tier"));
}

#[test]
fn enforce_tier_model_name_two_part_not_treated_as_full_spec() {
    // provider/model format (2 parts) should NOT be delegated to spec-level check
    let cfg = config_with_tiers(&["opencode/google/gemini-2.5-pro/medium"]);
    // This should pass via model-name matching, not fail as a "spec with wrong tool"
    assert!(
        cfg.enforce_tier_model_name("opencode", Some("google/gemini-2.5-pro"))
            .is_ok()
    );
}

#[test]
fn enforce_tier_model_name_three_part_rejected_as_model_name() {
    // 3-part value like "provider/model/extra" is not a valid 4-part spec,
    // so it falls through to model-name check and gets rejected.
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    let err = cfg
        .enforce_tier_model_name("codex", Some("openai/gpt-5.3-codex/high"))
        .unwrap_err();
    assert!(err.to_string().contains("not configured in any tier"));
}

// ---------------------------------------------------------------------------
// enforce_thinking_level
// ---------------------------------------------------------------------------

#[test]
fn enforce_thinking_level_empty_tiers_allows_all() {
    let cfg = ProjectConfig {
        tiers: HashMap::new(),
        ..config_with_tiers(&[])
    };
    assert!(cfg.enforce_thinking_level(Some("medium")).is_ok());
}

#[test]
fn enforce_thinking_level_none_allows() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    assert!(cfg.enforce_thinking_level(None).is_ok());
}

#[test]
fn enforce_thinking_level_configured_ok() {
    let cfg = config_with_tiers(&[
        "codex/openai/gpt-5.3-codex/high",
        "claude-code/anthropic/sonnet-4.5/xhigh",
    ]);
    assert!(cfg.enforce_thinking_level(Some("high")).is_ok());
    assert!(cfg.enforce_thinking_level(Some("xhigh")).is_ok());
}

#[test]
fn enforce_thinking_level_unconfigured_rejected() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    let err = cfg.enforce_thinking_level(Some("medium")).unwrap_err();
    assert!(err.to_string().contains("not configured in any tier"));
    assert!(err.to_string().contains("medium"));
    assert!(err.to_string().contains("--force-override-user-config"));
}

#[test]
fn enforce_thinking_level_case_insensitive() {
    let cfg = config_with_tiers(&["codex/openai/gpt-5.3-codex/high"]);
    assert!(cfg.enforce_thinking_level(Some("HIGH")).is_ok());
    assert!(cfg.enforce_thinking_level(Some("High")).is_ok());
}

#[test]
fn enforce_thinking_level_lists_configured_levels() {
    let cfg = config_with_tiers(&[
        "codex/openai/gpt-5.3-codex/high",
        "claude-code/anthropic/sonnet-4.5/xhigh",
    ]);
    let err = cfg.enforce_thinking_level(Some("low")).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("high"));
    assert!(msg.contains("xhigh"));
}

// ---------------------------------------------------------------------------
// tier_contains_tool
// ---------------------------------------------------------------------------

fn config_with_multi_tiers() -> ProjectConfig {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-2-standard".to_string(),
        TierConfig {
            description: "standard tier".to_string(),
            models: vec![
                "gemini-cli/google/gemini-2.5-pro/medium".to_string(),
                "codex/openai/gpt-5.3-codex/high".to_string(),
            ],
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    tiers.insert(
        "tier-4-critical".to_string(),
        TierConfig {
            description: "critical tier".to_string(),
            models: vec![
                "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
                "claude-code/anthropic/default/xhigh".to_string(),
            ],
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
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
        tool_aliases: HashMap::new(),
        preferences: None,
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

#[test]
fn tier_contains_tool_present() {
    let cfg = config_with_multi_tiers();
    assert!(cfg.tier_contains_tool("tier-4-critical", "gemini-cli"));
    assert!(cfg.tier_contains_tool("tier-4-critical", "claude-code"));
    assert!(cfg.tier_contains_tool("tier-2-standard", "codex"));
}

#[test]
fn tier_contains_tool_absent() {
    let cfg = config_with_multi_tiers();
    assert!(!cfg.tier_contains_tool("tier-4-critical", "opencode"));
    assert!(!cfg.tier_contains_tool("tier-4-critical", "codex"));
    assert!(!cfg.tier_contains_tool("tier-2-standard", "claude-code"));
}

#[test]
fn tier_contains_tool_unknown_tier() {
    let cfg = config_with_multi_tiers();
    assert!(!cfg.tier_contains_tool("nonexistent-tier", "gemini-cli"));
}

// ---------------------------------------------------------------------------
// list_tools_in_tier
// ---------------------------------------------------------------------------

#[test]
fn list_tools_in_tier_returns_all() {
    let cfg = config_with_multi_tiers();
    let tools = cfg.list_tools_in_tier("tier-4-critical");
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].0, "gemini-cli");
    assert_eq!(tools[1].0, "claude-code");
}

#[test]
fn list_tools_in_tier_unknown_returns_empty() {
    let cfg = config_with_multi_tiers();
    let tools = cfg.list_tools_in_tier("nonexistent");
    assert!(tools.is_empty());
}

// ---------------------------------------------------------------------------
// find_tiers_for_tool
// ---------------------------------------------------------------------------

#[test]
fn find_tiers_for_tool_present_in_multiple() {
    let cfg = config_with_multi_tiers();
    let tiers = cfg.find_tiers_for_tool("gemini-cli");
    assert_eq!(tiers.len(), 2);
    let tier_names: Vec<&str> = tiers.iter().map(|(n, _)| n.as_str()).collect();
    assert!(tier_names.contains(&"tier-2-standard"));
    assert!(tier_names.contains(&"tier-4-critical"));
}

#[test]
fn find_tiers_for_tool_absent() {
    let cfg = config_with_multi_tiers();
    let tiers = cfg.find_tiers_for_tool("opencode");
    assert!(tiers.is_empty());
}

// ---------------------------------------------------------------------------
// suggest_compatible_alternatives
// ---------------------------------------------------------------------------

#[test]
fn suggest_compatible_alternatives_includes_available_tools() {
    let cfg = config_with_multi_tiers();
    let msg = cfg.suggest_compatible_alternatives("opencode", "tier-4-critical");
    // Should list tools in tier-4-critical
    assert!(msg.contains("gemini-cli"));
    assert!(msg.contains("claude-code"));
    assert!(msg.contains("Available tools in tier 'tier-4-critical'"));
}

#[test]
fn suggest_compatible_alternatives_includes_compatible_tiers() {
    let cfg = config_with_multi_tiers();
    let msg = cfg.suggest_compatible_alternatives("codex", "tier-4-critical");
    // codex is in tier-2-standard
    assert!(msg.contains("tier-2-standard"));
    assert!(msg.contains("Tiers containing 'codex'"));
}

#[test]
fn suggest_compatible_alternatives_includes_action_hints() {
    let cfg = config_with_multi_tiers();
    let msg = cfg.suggest_compatible_alternatives("opencode", "tier-4-critical");
    assert!(msg.contains("Auto-select:"));
    assert!(msg.contains("--force-ignore-tier-setting"));
}

#[test]
fn suggest_compatible_alternatives_tool_not_in_any_tier() {
    let cfg = config_with_multi_tiers();
    let msg = cfg.suggest_compatible_alternatives("opencode", "tier-4-critical");
    // opencode is not in any tier, so no "Tiers containing" section
    assert!(!msg.contains("Tiers containing 'opencode'"));
    // But should still have available tools and action hints
    assert!(msg.contains("Available tools in tier"));
    assert!(msg.contains("Suggestions:"));
}

// ---------------------------------------------------------------------------
// valid tool+tier combos still work (regression guard)
// ---------------------------------------------------------------------------

#[test]
fn enforce_tier_whitelist_valid_tool_in_tier_ok() {
    let cfg = config_with_multi_tiers();
    assert!(cfg.enforce_tier_whitelist("gemini-cli", None).is_ok());
    assert!(cfg.enforce_tier_whitelist("claude-code", None).is_ok());
    assert!(cfg.enforce_tier_whitelist("codex", None).is_ok());
}

#[test]
fn enforce_tier_whitelist_tool_not_in_any_tier_shows_hint() {
    let cfg = config_with_multi_tiers();
    let err = cfg.enforce_tier_whitelist("opencode", None).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("not configured in any tier"));
    assert!(msg.contains("--force-ignore-tier-setting"));
}
