use super::super::routing::{resolve_run_effective_tier, resolve_tier_failover_tool_filter};
use crate::run_cmd_tool_selection::resolve_tool_by_strategy;
use crate::run_helpers::compound_tier_selects_tool;
use crate::test_env_lock::ScopedTestEnvVar;
use chrono::Utc;
use csa_config::{GlobalConfig, ProjectConfig, ProjectMeta, TierConfig, TierStrategy, ToolConfig};
use csa_core::types::{ToolArg, ToolName};
use std::collections::HashMap;
use tempfile::TempDir;

fn make_config_with_tier_models(tier_name: &str, models: &[&str]) -> ProjectConfig {
    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: Default::default(),
        acp: Default::default(),
        tools: csa_config::global::all_known_tools()
            .iter()
            .map(|tool| {
                let name = tool.as_str();
                (
                    name.to_string(),
                    ToolConfig {
                        enabled: name == "codex",
                        ..Default::default()
                    },
                )
            })
            .collect(),
        review: None,
        debate: None,
        tiers: HashMap::from([(
            tier_name.to_string(),
            TierConfig {
                description: "test tier".to_string(),
                models: models.iter().map(|model| (*model).to_string()).collect(),
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        )]),
        tier_mapping: HashMap::from([("default".to_string(), tier_name.to_string())]),
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
fn compound_tier_tool_selector_sets_explicit_tool_failover_filter() {
    let _availability =
        ScopedTestEnvVar::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let tmp = TempDir::new().expect("tempdir");
    let config = make_config_with_tier_models(
        "tier-3-complex",
        &[
            "codex/openai/gpt-5.3-codex-spark/xhigh",
            "openai-compat/openai/gpt-5/high",
            "codex/openai/gpt-5.5/xhigh",
        ],
    );
    let global_config = GlobalConfig::default();

    let mut user_explicit_tool = false;
    let effective_tier = resolve_run_effective_tier(
        Some(&config),
        Some("tier-3-complex-codex"),
        None,
        None,
        None,
        None,
    )
    .expect("compound tier selector should resolve as an explicit tier string");
    user_explicit_tool |= compound_tier_selects_tool(effective_tier.as_deref(), Some(&config));

    let (effective_tier, compounded_tool) =
        crate::run_helpers::apply_compound_tier_selector_arg(effective_tier, None, Some(&config))
            .expect("compound selector should inject the tool");

    assert!(user_explicit_tool);
    assert_eq!(effective_tier.as_deref(), Some("tier-3-complex"));
    assert!(matches!(
        &compounded_tool,
        Some(ToolArg::Specific(ToolName::Codex))
    ));

    let resolved_tool_arg = compounded_tool.expect("compound selector should provide a tool");
    let strategy = resolved_tool_arg.clone().into_strategy();
    let worker = resolve_tool_by_strategy(
        &strategy,
        None,
        None,
        None,
        Some(&config),
        &global_config,
        tmp.path(),
        false,
        false,
        true,
        effective_tier.as_deref(),
        false,
    )
    .expect("compound-selected codex should resolve inside the tier");
    let tier_failover_tool_filter = resolve_tier_failover_tool_filter(
        user_explicit_tool,
        effective_tier.is_some(),
        false,
        &resolved_tool_arg,
    );

    assert_eq!(worker.tool, ToolName::Codex);
    assert_eq!(tier_failover_tool_filter, Some(ToolName::Codex));
    assert_eq!(
        resolve_tier_failover_tool_filter(
            user_explicit_tool,
            effective_tier.is_some(),
            true,
            &resolved_tool_arg,
        ),
        None,
        "--allow-fallback must opt explicit tool tier runs back into cross-tool fallback"
    );
}

#[test]
fn explicit_auto_and_any_available_tier_runs_do_not_set_failover_tool_filter() {
    let _availability =
        ScopedTestEnvVar::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let tmp = TempDir::new().expect("tempdir");
    let config = make_config_with_tier_models(
        "tier-3-complex",
        &[
            "codex/openai/gpt-5.3-codex-spark/xhigh",
            "openai-compat/openai/gpt-5/high",
            "codex/openai/gpt-5.5/xhigh",
        ],
    );
    let global_config = GlobalConfig::default();

    for (label, tool_arg) in [
        ("auto", ToolArg::Auto),
        ("any-available", ToolArg::AnyAvailable),
    ] {
        let mut user_explicit_tool = true;
        let effective_tier = resolve_run_effective_tier(
            Some(&config),
            Some("tier-3-complex"),
            None,
            None,
            None,
            None,
        )
        .expect("tier selector should resolve");
        user_explicit_tool |= compound_tier_selects_tool(effective_tier.as_deref(), Some(&config));

        let (effective_tier, selected_tool_arg) =
            crate::run_helpers::apply_compound_tier_selector_arg(
                effective_tier,
                Some(tool_arg),
                Some(&config),
            )
            .expect("plain tier selector should not rewrite auto/any-available");
        let resolved_tool_arg = selected_tool_arg.expect("explicit tool arg should remain present");
        let strategy = resolved_tool_arg.clone().into_strategy();
        let worker = resolve_tool_by_strategy(
            &strategy,
            None,
            None,
            None,
            Some(&config),
            &global_config,
            tmp.path(),
            false,
            false,
            true,
            effective_tier.as_deref(),
            false,
        )
        .expect("auto/any-available tier run should resolve an initial worker");
        let tier_failover_tool_filter = resolve_tier_failover_tool_filter(
            user_explicit_tool,
            effective_tier.is_some(),
            false,
            &resolved_tool_arg,
        );

        assert_eq!(
            worker.tool,
            ToolName::Codex,
            "{label} should initially resolve the first executable tier candidate"
        );
        assert_eq!(
            tier_failover_tool_filter, None,
            "{label} should preserve cross-tool tier failover"
        );
    }
}
