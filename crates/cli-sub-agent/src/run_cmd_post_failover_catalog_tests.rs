use super::*;
use csa_config::{ProjectConfig, ProjectMeta, TierConfig, TierStrategy};
use std::collections::HashMap;

fn failover_config() -> ProjectConfig {
    let mut tiers = HashMap::new();
    tiers.insert(
        "catalog-tier".to_string(),
        TierConfig {
            description: "catalog failover".to_string(),
            models: vec![
                "codex/test-provider/model-a/high".to_string(),
                "codex/test-provider/model-b/high".to_string(),
                "codex/test-provider/model-c/high".to_string(),
            ],
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta::default(),
        resources: Default::default(),
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

fn closed_catalog_without_middle_candidate() -> csa_config::EffectiveModelCatalog {
    csa_config::EffectiveModelCatalog::from_toml_str(
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "test-provider"
model = "model-a"
reasoning_efforts = ["high"]

[[model_catalog.entries]]
tool = "codex"
provider = "test-provider"
model = "model-c"
reasoning_efforts = ["high"]
"#,
        "run failover test",
    )
    .expect("test catalog")
}

#[test]
fn failover_admits_configured_unverified_middle_candidate() {
    let _available = crate::test_env_lock::ScopedTestEnvVar::set(
        crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV,
        "1",
    );
    let config = failover_config();
    let mut catalog = closed_catalog_without_middle_candidate();
    catalog
        .register_configured_spec(
            "codex",
            "test-provider",
            "model-b",
            "high",
            csa_config::CatalogProvenance::Inline {
                source: "effective tier config".to_string(),
                key: "tiers.catalog-tier.models[1]".to_string(),
            },
        )
        .expect("register configured failover candidate");
    let mut tried_tools = vec!["codex".to_string()];
    let mut tried_specs = vec!["codex/test-provider/model-a/high".to_string()];

    let action = decide_available_failover(
        FailoverAvailabilityRequest {
            failed_tool: "codex",
            task_type: "default",
            resolved_tier_name: Some("catalog-tier"),
            required_tool: None,
            task_needs_edit: Some(true),
            session_state: None,
            exhausted_providers: &[],
            config: &config,
            global_config: None,
            original_error: "rate limited",
            model_catalog: &catalog,
        },
        FailoverAvailabilityState {
            tried_tools: &mut tried_tools,
            tried_specs: &mut tried_specs,
        },
    )
    .expect("failover decision");

    match action {
        RateLimitAction::Retry {
            new_tool,
            new_model_spec,
            ..
        } => {
            assert_eq!(new_tool, csa_core::types::ToolName::Codex);
            assert_eq!(
                new_model_spec.as_deref(),
                Some("codex/test-provider/model-b/high")
            );
        }
        _ => panic!("expected catalog-admitted retry"),
    }
}
