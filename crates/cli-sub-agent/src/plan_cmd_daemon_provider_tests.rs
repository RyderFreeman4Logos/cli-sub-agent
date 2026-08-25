use super::*;
use std::collections::HashMap;

#[test]
fn detached_outer_plan_preserves_native_provider_for_nested_pr_bot() {
    let provider = native_parent_provider(Some("codex"), None).expect("native provider");
    let outer_startup =
        crate::startup_env::StartupSubtreeEnv::default().with_parent_model_provider(provider);
    let mut daemon_env = HashMap::new();
    outer_startup.apply_to_child_env(&mut daemon_env);

    let daemon_startup = crate::startup_env::StartupSubtreeEnv::from_values(HashMap::from([(
        csa_core::env::CSA_PARENT_MODEL_PROVIDER_ENV_KEY,
        daemon_env[csa_core::env::CSA_PARENT_MODEL_PROVIDER_ENV_KEY].clone(),
    )]));
    let bash_env: HashMap<_, _> = daemon_startup
        .to_csa_child_contract_env_vars()
        .into_iter()
        .collect();
    let nested_startup = crate::startup_env::StartupSubtreeEnv::from_values(HashMap::from([(
        csa_core::env::CSA_PARENT_MODEL_PROVIDER_ENV_KEY,
        bash_env[csa_core::env::CSA_PARENT_MODEL_PROVIDER_ENV_KEY].clone(),
    )]));
    let mut nested_vars = Vec::new();

    inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut nested_vars,
        nested_startup.parent_model_provider(),
    );

    assert_eq!(nested_vars, ["CSA_MODEL_PROVIDER=openai"]);
}

#[test]
fn native_codex_pr_bot_entrypoint_injects_parent_provider() {
    let mut vars = Vec::new();

    inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut vars,
        native_parent_provider(Some("codex"), None).as_deref(),
    );

    assert_eq!(vars, ["CSA_MODEL_PROVIDER=openai"]);
}

#[test]
fn native_claude_file_entrypoint_injects_parent_provider() {
    let mut vars = Vec::new();

    inject_pr_bot_parent_provider(
        &Some("patterns/pr-bot/workflow.toml".to_string()),
        &None,
        &mut vars,
        native_parent_provider(Some("claude-code"), None).as_deref(),
    );

    assert_eq!(vars, ["CSA_MODEL_PROVIDER=claude"]);
}

#[test]
fn explicit_pr_bot_provider_key_wins_unchanged() {
    let mut vars = vec!["CSA_MODEL_PROVIDER=anthropic".to_string()];

    inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut vars,
        native_parent_provider(Some("claude-code"), None).as_deref(),
    );

    assert_eq!(vars, ["CSA_MODEL_PROVIDER=anthropic"]);
}

#[test]
fn hermes_config_provider_requires_detected_hermes_parent() {
    let mut native_vars = Vec::new();
    inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut native_vars,
        native_parent_provider(Some("opencode"), Some("xai")).as_deref(),
    );
    assert!(native_vars.is_empty());

    let mut hermes_vars = Vec::new();
    inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut hermes_vars,
        native_parent_provider(Some("hermes"), Some("xai")).as_deref(),
    );
    assert_eq!(hermes_vars, ["CSA_MODEL_PROVIDER=xai"]);
}

#[test]
fn every_pr_bot_wait_routes_through_the_provider_qualified_helper() {
    let workflow = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../patterns/pr-bot/workflow.toml"),
    )
    .expect("read pr-bot workflow");

    assert_eq!(
        workflow.matches("session-wait-until-done.sh").count(),
        5,
        "every documented pr-bot wait must route through the shared provider helper"
    );
    assert!(
        !workflow.contains("csa session wait"),
        "pr-bot must not bypass the provider-qualified helper"
    );
}
