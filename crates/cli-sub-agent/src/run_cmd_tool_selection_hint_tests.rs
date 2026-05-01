use super::*;
use crate::test_env_lock::ScopedTestEnvVar;
use clap::Parser;
use csa_config::{
    GlobalConfig, ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, TierStrategy, ToolConfig,
};
use csa_core::types::ToolName;
use std::collections::HashMap;
use tempfile::TempDir;

fn config_with_enabled_tiers(
    enabled_tools: &[&str],
    tiers: &[(&str, Vec<&str>)],
    tier_mapping: &[(&str, &str)],
) -> ProjectConfig {
    let mut tools = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        let name = tool.as_str();
        tools.insert(
            name.to_string(),
            ToolConfig {
                enabled: enabled_tools.contains(&name),
                ..Default::default()
            },
        );
    }

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
        tiers: tiers
            .iter()
            .map(|(name, models)| {
                (
                    (*name).to_string(),
                    TierConfig {
                        description: "Test tier".to_string(),
                        models: models.iter().map(|model| (*model).to_string()).collect(),
                        strategy: TierStrategy::default(),
                        token_budget: None,
                        max_turns: None,
                    },
                )
            })
            .collect(),
        tier_mapping: tier_mapping
            .iter()
            .map(|(label, tier_name)| ((*label).to_string(), (*tier_name).to_string()))
            .collect(),
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
fn csa_run_tool_hint_difficulty_resolves_quick_tier() {
    let _tool_availability =
        ScopedTestEnvVar::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let tmp = TempDir::new().expect("tempdir");
    let cli = crate::cli::Cli::try_parse_from([
        "csa",
        "run",
        "--tool",
        "claude",
        "--hint-difficulty",
        "quick_question",
        "answer briefly",
    ])
    .expect("run CLI should parse hint difficulty");
    let (tool_arg, hint_difficulty) = match cli.command {
        crate::cli::Commands::Run {
            tool,
            hint_difficulty,
            ..
        } => (tool, hint_difficulty),
        _ => panic!("expected run command"),
    };
    let config = config_with_enabled_tiers(
        &["claude-code"],
        &[(
            "tier-1-quick",
            vec!["claude-code/anthropic/claude-haiku/low"],
        )],
        &[("quick_question", "tier-1-quick")],
    );
    let effective_tier = crate::difficulty_routing::resolve_effective_tier_with_difficulty_hint(
        Some(&config),
        None,
        None,
        hint_difficulty.as_deref(),
        None,
    )
    .expect("hint should map through tier_mapping");
    let strategy = tool_arg.expect("tool should parse").into_strategy();
    let resolution = resolve_tool_by_strategy(
        &strategy,
        None,
        None,
        None, // thinking
        Some(&config),
        &GlobalConfig::default(),
        tmp.path(),
        false,
        false,
        false,
        effective_tier.as_deref(),
        false,
    )
    .expect("hint-selected tier should route requested tool");

    assert_eq!(resolution.tool, ToolName::ClaudeCode);
    assert_eq!(
        resolution.model_spec.as_deref(),
        Some("claude-code/anthropic/claude-haiku/low")
    );
    assert_eq!(
        resolution.resolved_tier_name.as_deref(),
        Some("tier-1-quick")
    );
}
