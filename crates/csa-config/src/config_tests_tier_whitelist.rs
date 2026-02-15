use super::*;

fn config_with_tiers(tier_models: &[&str]) -> ProjectConfig {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-2-standard".to_string(),
        TierConfig {
            description: "test tier".to_string(),
            models: tier_models.iter().map(|s| s.to_string()).collect(),
            token_budget: None,
            max_turns: None,
        },
    );
    ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
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
