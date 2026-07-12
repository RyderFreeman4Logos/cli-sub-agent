use super::*;
use crate::config::{
    CURRENT_SCHEMA_VERSION, ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, TierStrategy,
};
use chrono::Utc;
use std::collections::HashMap;

#[test]
fn tier_validate_admits_configured_unverified_cross_provider_model() {
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
    };

    let mut catalog = EffectiveModelCatalog::shipped().expect("shipped catalog");
    crate::configured_models::register_configured_tier_specs(
        &mut catalog,
        &config,
        std::path::Path::new("/tmp/configured-model.toml"),
    )
    .expect("register configured tier spec");
    let result = validate_loaded_config(Some(config), &catalog);
    assert!(
        result.is_ok(),
        "active config model must warn instead of blocking: {result:?}"
    );
}

#[test]
fn tier_validate_rejects_invalid_thinking_budget() {
    fn config_with_model_spec(model_spec: &str) -> ProjectConfig {
        let mut tiers = HashMap::new();
        tiers.insert(
            "budget-tier".to_string(),
            TierConfig {
                description: "Budget validation".to_string(),
                models: vec![model_spec.to_string()],
                strategy: TierStrategy::default(),

                token_budget: None,
                max_turns: None,
            },
        );

        ProjectConfig {
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

    let catalog = EffectiveModelCatalog::shipped().expect("shipped catalog");
    let invalid_spec = "codex/openai/gpt-5.5/minimal";
    let result = validate_loaded_config(Some(config_with_model_spec(invalid_spec)), &catalog);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("minimal"),
        "Expected offending budget in error, got: {err_msg}"
    );
    assert!(
        err_msg.contains("thinking_budget") || err_msg.contains(invalid_spec),
        "Expected thinking_budget field or full model spec in error, got: {err_msg}"
    );

    let valid_spec = "codex/openai/gpt-5.5/xhigh";
    let result = validate_loaded_config(Some(config_with_model_spec(valid_spec)), &catalog);
    assert!(result.is_ok(), "valid tier model spec should pass");
}
