use super::*;
use std::collections::HashMap;

fn config_with_tier_4_critical() -> ProjectConfig {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-4-critical".to_string(),
        TierConfig {
            description: "test".to_string(),
            models: vec!["codex/openai/gpt-5.5/xhigh".to_string()],
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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    }
}

#[test]
fn suggest_tier_migrates_legacy_tier_4_hard_selector() {
    let config = config_with_tier_4_critical();
    assert_eq!(
        config.resolve_tier_selector("tier-4-hard"),
        None,
        "legacy selector should not silently resolve to a different tier"
    );
    assert_eq!(
        config.suggest_tier("tier-4-hard"),
        Some("tier-4-critical".to_string())
    );
}
