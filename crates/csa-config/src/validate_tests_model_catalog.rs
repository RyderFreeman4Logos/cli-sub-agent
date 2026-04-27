use super::*;
use crate::config::{
    CURRENT_SCHEMA_VERSION, ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, TierStrategy,
};
use chrono::Utc;
use std::collections::HashMap;

#[test]
fn tier_validate_rejects_cross_provider_opencode_spec() {
    let mut tiers = HashMap::new();
    tiers.insert(
        "bad-opencode-tier".to_string(),
        TierConfig {
            description: "Cross-provider opencode model".to_string(),
            models: vec!["opencode/openai/gemini-2.5-pro/high".to_string()],
            strategy: TierStrategy::default(),

            token_budget: None,
            max_turns: None,
        },
    );

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
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
    };

    let result = validate_loaded_config(Some(config));
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("unknown model"),
        "Expected 'unknown model' in error, got: {err_msg}"
    );
    assert!(err_msg.contains("gemini-2.5-pro"));
    assert!(err_msg.contains("provider 'openai'"));
    assert!(err_msg.contains("gpt-5"));
    assert!(!err_msg.contains("claude-opus-4-7"));
}
