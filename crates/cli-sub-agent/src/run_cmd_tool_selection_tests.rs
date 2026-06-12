use super::*;
use std::collections::HashMap;
use std::fs;
use tempfile::TempDir;

use csa_config::{
    GlobalConfig, ProjectConfig, ProjectMeta, ResourcesConfig, TierStrategy, ToolConfig,
};
use csa_core::types::ToolSelectionStrategy;

fn config_with_openai_compat_tiers(
    tiers: &[(&str, Vec<&str>)],
    tier_mapping: &[(&str, &str)],
) -> ProjectConfig {
    let mut tools = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        let name = tool.as_str();
        tools.insert(
            name.to_string(),
            ToolConfig {
                enabled: name == "openai-compat",
                base_url: (name == "openai-compat").then(|| "http://localhost:8317".to_string()),
                api_key: (name == "openai-compat").then(|| "test-key".to_string()),
                ..Default::default()
            },
        );
    }

    let tiers = tiers
        .iter()
        .map(|(name, models)| {
            (
                (*name).to_string(),
                csa_config::config::TierConfig {
                    description: "Test tier".to_string(),
                    models: models.iter().map(|model| (*model).to_string()).collect(),
                    strategy: TierStrategy::default(),
                    token_budget: None,
                    max_turns: None,
                },
            )
        })
        .collect();
    let tier_mapping = tier_mapping
        .iter()
        .map(|(selector, tier_name)| ((*selector).to_string(), (*tier_name).to_string()))
        .collect();

    ProjectConfig {
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
        tiers,
        tier_mapping,
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
        filesystem_sandbox: Default::default(),
    }
}

#[test]
fn resolve_skill_and_prompt_injects_workspace_scope_guard() {
    let tmp = TempDir::new().expect("tempdir");
    let skill_dir = tmp.path().join(".csa").join("skills").join("demo");
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    fs::write(skill_dir.join("SKILL.md"), "demo skill body").expect("write SKILL.md");

    let resolved = resolve_skill_and_prompt(
        Some("demo"),
        Some("user task".to_string()),
        None,
        None,
        None,
        tmp.path(),
    )
    .expect("resolve skill prompt");

    assert!(
        resolved
            .prompt_text
            .contains("<skill-mode>executor</skill-mode>")
    );
    assert!(resolved.prompt_text.contains("<workspace-scope root=\""));
    assert!(
        resolved
            .prompt_text
            .contains("STRICT SCOPE: Only read/write files under this root.")
    );
    assert!(resolved.prompt_text.contains("demo skill body"));
    assert!(resolved.prompt_text.contains("user task"));
}

#[test]
fn resolve_tool_by_strategy_records_canonical_cli_tier_name() {
    let tmp = TempDir::new().expect("tempdir");
    let config = config_with_openai_compat_tiers(
        &[(
            "tier-2-standard",
            vec!["openai-compat/openai/gpt-5-codex/high"],
        )],
        &[("fast", "tier-2-standard")],
    );

    let resolution = resolve_tool_by_strategy(
        &ToolSelectionStrategy::AnyAvailable,
        None,
        None,
        None, // thinking
        Some(&config),
        &GlobalConfig::default(),
        tmp.path(),
        false,
        false,
        false,
        Some("fast"),
        false,
    )
    .expect("resolve tool by CLI tier");

    assert_eq!(
        resolution.resolved_tier_name.as_deref(),
        Some("tier-2-standard")
    );
}

#[test]
fn resolve_tool_by_strategy_records_config_default_tier_name() {
    let tmp = TempDir::new().expect("tempdir");
    let config = config_with_openai_compat_tiers(
        &[(
            "tier-3-complex",
            vec!["openai-compat/openai/gpt-5-codex/high"],
        )],
        &[("default", "tier-3-complex")],
    );

    let resolution = resolve_tool_by_strategy(
        &ToolSelectionStrategy::AnyAvailable,
        None,
        None,
        None, // thinking
        Some(&config),
        &GlobalConfig::default(),
        tmp.path(),
        false,
        false,
        false,
        None,
        false,
    )
    .expect("resolve tool by config default tier");

    assert_eq!(
        resolution.resolved_tier_name.as_deref(),
        Some("tier-3-complex")
    );
}
