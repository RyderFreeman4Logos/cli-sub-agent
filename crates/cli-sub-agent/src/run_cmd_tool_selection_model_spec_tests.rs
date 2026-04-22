use super::resolve_tool_by_strategy;
use csa_config::global::DefaultsConfig;
use csa_config::{
    GlobalConfig, ProjectConfig, ProjectMeta, ResourcesConfig, TierStrategy, ToolConfig,
};
use csa_core::types::ToolSelectionStrategy;
use std::collections::HashMap;
use tempfile::TempDir;

#[test]
fn resolve_tool_by_strategy_model_spec_disables_default_tier_and_runtime_fallbacks() {
    let tmp = TempDir::new().expect("tempdir");
    let mut tools = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        let name = tool.as_str();
        tools.insert(
            name.to_string(),
            ToolConfig {
                enabled: matches!(name, "codex" | "openai-compat" | "gemini-cli"),
                ..Default::default()
            },
        );
    }
    let config = ProjectConfig {
        schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: chrono::Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers: HashMap::from([(
            "tier-3-complex".to_string(),
            csa_config::config::TierConfig {
                description: "Test tier".to_string(),
                models: vec!["openai-compat/openai/gpt-5-codex/high".to_string()],
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        )]),
        tier_mapping: HashMap::from([("default".to_string(), "tier-3-complex".to_string())]),
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
    let global_config = GlobalConfig {
        defaults: DefaultsConfig {
            tool: Some("codex".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let resolution = resolve_tool_by_strategy(
        &ToolSelectionStrategy::HeterogeneousPreferred,
        Some("codex/openai/gpt-5.4/high"),
        None,
        Some(&config),
        &global_config,
        tmp.path(),
        false,
        false,
        false,
        None,
        false,
    )
    .expect("resolve tool by explicit model_spec");

    assert_eq!(
        resolution.model_spec.as_deref(),
        Some("codex/openai/gpt-5.4/high")
    );
    assert!(resolution.resolved_tier_name.is_none());
    assert!(resolution.runtime_fallback_candidates.is_empty());
}
