use super::*;
use std::collections::HashMap;

fn default_wait_config() -> csa_config::KvCacheConfig {
    toml::from_str::<csa_config::GlobalConfig>(&csa_config::GlobalConfig::default_template())
        .expect("default generated config")
        .kv_cache
}

fn trusted_parent_model_spec(model_spec: &str) -> crate::startup_env::StartupSubtreeEnv {
    crate::startup_env::StartupSubtreeEnv::from_values(HashMap::from([
        (csa_core::env::CSA_DEPTH_ENV_KEY, "1".to_string()),
        (
            csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY,
            "1".to_string(),
        ),
        (
            csa_core::env::CSA_SESSION_ID_ENV_KEY,
            "01M0PARENT".to_string(),
        ),
        (
            csa_core::env::CSA_SESSION_DIR_ENV_KEY,
            "/tmp/01M0PARENT".to_string(),
        ),
        (
            csa_core::env::CSA_PROJECT_ROOT_ENV_KEY,
            "/tmp/project".to_string(),
        ),
        (
            csa_core::env::CSA_MODEL_SPEC_ENV_KEY,
            model_spec.to_string(),
        ),
        (
            csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
            "1".to_string(),
        ),
    ]))
    .with_trusted_inherited_model_pin(model_spec.to_string(), true, false)
}

#[test]
fn detached_outer_plan_preserves_native_provider_for_nested_pr_bot() {
    let startup_env = trusted_parent_model_spec("codex/XAI/gpt-5.5/xhigh");
    let provider =
        native_parent_provider(Some("codex"), &startup_env, None).expect("native provider");
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
        &default_wait_config(),
    )
    .expect("nested provider must be accepted");

    assert_eq!(nested_vars, ["CSA_MODEL_PROVIDER=xai"]);
}

#[test]
fn native_tool_without_proven_routing_fails_closed() {
    let startup_env = crate::startup_env::StartupSubtreeEnv::default();

    for tool in ["codex", "claude-code"] {
        let mut vars = Vec::new();
        let error = inject_pr_bot_parent_provider(
            &None,
            &Some("pr-bot".to_string()),
            &mut vars,
            native_parent_provider(Some(tool), &startup_env, None).as_deref(),
            &default_wait_config(),
        )
        .expect_err("unproven native routing must fail before the plan starts");

        assert!(error.to_string().contains("CSA_MODEL_PROVIDER"));
        assert!(vars.is_empty());
    }
}

#[test]
fn explicit_pr_bot_provider_key_wins_unchanged() {
    let mut vars = vec!["CSA_MODEL_PROVIDER=anthropic".to_string()];
    let startup_env = crate::startup_env::StartupSubtreeEnv::default();
    let mut config = default_wait_config();
    config.provider_ttls.0.insert("anthropic".to_string(), 17);

    inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut vars,
        native_parent_provider(Some("claude-code"), &startup_env, None).as_deref(),
        &config,
    )
    .expect("explicit provider must be accepted");

    assert_eq!(vars, ["CSA_MODEL_PROVIDER=anthropic"]);

    let mut default_vars = vec!["CSA_MODEL_PROVIDER=xai".to_string()];
    inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut default_vars,
        None,
        &default_wait_config(),
    )
    .expect("default configured provider must be accepted");
    assert_eq!(default_vars, ["CSA_MODEL_PROVIDER=xai"]);
}

#[test]
fn foreground_pr_bot_normalizes_the_last_repeated_provider_assignment() {
    let mut vars = vec![
        "CSA_MODEL_PROVIDER=anthropic".to_string(),
        "CSA_MODEL_PROVIDER=google".to_string(),
    ];

    let provider = inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut vars,
        None,
        &default_wait_config(),
    )
    .expect("effective provider must be accepted");

    assert_eq!(provider.as_deref(), Some("other"));
    assert_eq!(
        vars,
        ["CSA_MODEL_PROVIDER=anthropic", "CSA_MODEL_PROVIDER=other"]
    );

    let mut vars = vec![
        "CSA_MODEL_PROVIDER=anthropic".to_string(),
        "CSA_MODEL_PROVIDER=   ".to_string(),
    ];
    inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut vars,
        None,
        &default_wait_config(),
    )
    .expect_err("an empty effective provider must be rejected");
}

#[test]
fn hermes_config_provider_requires_detected_hermes_parent() {
    let startup_env = crate::startup_env::StartupSubtreeEnv::default();
    assert_eq!(
        native_parent_provider(Some("opencode"), &startup_env, Some("xai")),
        None,
        "an unrelated Hermes config must not become OpenCode's provider"
    );

    let mut hermes_vars = Vec::new();
    inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut hermes_vars,
        native_parent_provider(Some("hermes"), &startup_env, Some("xai")).as_deref(),
        &default_wait_config(),
    )
    .expect("Hermes provider must be accepted");
    assert_eq!(hermes_vars, ["CSA_MODEL_PROVIDER=xai"]);
}

#[test]
fn cross_provider_native_plan_entries_use_the_trusted_parent_model_spec() {
    for (tool, model_spec, expected_provider) in [
        ("codex", "codex/XAI/grok-code-fast-1/xhigh", "xai"),
        (
            "claude-code",
            "claude-code/anthropic/claude-sonnet-4/high",
            "claude",
        ),
        ("opencode", "opencode/openai/gpt-5/xhigh", "openai"),
        (
            "opencode",
            "opencode/anthropic/claude-sonnet-4/high",
            "claude",
        ),
        (
            "antigravity-cli",
            "antigravity-cli/google/gemini-3.1-pro/high",
            "other",
        ),
    ] {
        let startup_env = trusted_parent_model_spec(model_spec);
        let mut vars = Vec::new();

        inject_pr_bot_parent_provider(
            &None,
            &Some("pr-bot".to_string()),
            &mut vars,
            native_parent_provider(Some(tool), &startup_env, None).as_deref(),
            &default_wait_config(),
        )
        .expect("canonical pr-bot entry must have an explicit provider before execution");

        assert_eq!(
            vars,
            [format!("CSA_MODEL_PROVIDER={expected_provider}")],
            "{tool} must pass a configured wait-TTL key"
        );
        let key = vars[0].split_once('=').expect("provider assignment").1;
        let provider = csa_config::parse_model_provider(key).expect("normalized provider");
        assert!(
            csa_config::provider_ttl(&provider, &default_wait_config()).is_some(),
            "{key} must resolve through the real wait-TTL resolver"
        );
    }
}

#[test]
fn canonical_pr_bot_entry_rejects_an_unknown_parent_provider_before_any_wait() {
    let mut vars = Vec::new();
    let startup_env = crate::startup_env::StartupSubtreeEnv::default();

    let error = inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut vars,
        native_parent_provider(Some("opencode"), &startup_env, None).as_deref(),
        &default_wait_config(),
    )
    .expect_err("unknown cross-provider routing must fail before the plan starts");

    assert!(error.to_string().contains("CSA_MODEL_PROVIDER"));
    assert!(vars.is_empty());
}

#[test]
fn canonical_pr_bot_entry_rejects_unmapped_provider_before_plan_start() {
    let mut vars = Vec::new();

    let error = inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut vars,
        Some("unmapped"),
        &default_wait_config(),
    )
    .expect_err("unmapped provider must fail before the plan starts");

    assert!(error.to_string().contains("configured provider"));
    assert!(vars.is_empty());
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
