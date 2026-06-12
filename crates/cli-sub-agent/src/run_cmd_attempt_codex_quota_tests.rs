use crate::run_cmd_post::{
    RateLimitAction, evaluate_error_rate_limit_failover,
    evaluate_error_rate_limit_failover_with_global_config, evaluate_rate_limit_failover,
};
use chrono::Utc;
use csa_config::global::GlobalToolConfig;
use csa_config::{GlobalConfig, ProjectConfig, ProjectMeta, TierConfig, TierStrategy};
use csa_process::ExecutionResult;
use std::{collections::HashMap, path::Path};

fn make_config(tier_name: &str, models: &[&str]) -> ProjectConfig {
    let mut tiers = HashMap::new();
    tiers.insert(
        tier_name.to_string(),
        TierConfig {
            description: "test".to_string(),
            models: models.iter().map(|model| (*model).to_string()).collect(),
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: Default::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
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

fn assert_retry_to(action: RateLimitAction, expected_tool: &str, expected_spec: &str) {
    match action {
        RateLimitAction::Retry {
            new_tool,
            new_model_spec,
        } => {
            assert_eq!(new_tool.as_str(), expected_tool);
            assert_eq!(new_model_spec.as_deref(), Some(expected_spec));
        }
        RateLimitAction::NoRateLimit => panic!("expected failover retry, got no rate limit"),
        RateLimitAction::ExhaustedFailovers { .. } => {
            panic!("expected failover retry, got exhausted failovers")
        }
    }
}

fn assume_tools_available() -> crate::test_env_lock::ScopedTestEnvVar {
    crate::test_env_lock::ScopedTestEnvVar::set(
        crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV,
        "1",
    )
}

fn global_openai_compat_env_config() -> GlobalConfig {
    let mut global_config = GlobalConfig::default();
    global_config.tools.insert(
        "openai-compat".to_string(),
        GlobalToolConfig {
            env: HashMap::from([
                (
                    "OPENAI_COMPAT_BASE_URL".to_string(),
                    "http://localhost:8317".to_string(),
                ),
                ("OPENAI_COMPAT_API_KEY".to_string(), "test-key".to_string()),
                ("OPENAI_COMPAT_MODEL".to_string(), "local-model".to_string()),
            ]),
            ..Default::default()
        },
    );
    global_config
}

fn codex_spark_quota_result() -> ExecutionResult {
    ExecutionResult {
        output: String::new(),
        stderr_output: "You've hit your usage limit for GPT-5.3-Codex-Spark. \
         Switch to another model now, or try again at Jun 11th, 2026 7:42 AM."
            .to_string(),
        summary: "Codex Spark quota reached".to_string(),
        exit_code: 1,
        peak_memory_mb: None,
        ..Default::default()
    }
}

#[test]
fn explicit_tool_in_tier_codex_model_scoped_quota_tries_next_codex_model() {
    let _assume = assume_tools_available();
    let config = make_config(
        "tier-3-complex",
        &[
            "codex/openai/gpt-5.3-codex-spark/xhigh",
            "codex/openai/gpt-5.5/xhigh",
            "claude-code/anthropic/claude-sonnet/high",
        ],
    );
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_error_rate_limit_failover(
        "codex",
        r#"Internal error: {"message":"You've hit your usage limit for GPT-5.3-Codex-Spark. Switch to another model now, or try again at Jun 11th, 2026 7:42 AM."}"#,
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        true,
        true,
        Some("tier-3-complex"),
        None,
        None,
        true,
        "recover from codex rate limit",
        Path::new("."),
        Some(&config),
        None,
        Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
        &mut fallback_chain,
        None,
    )
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/gpt-5.5/xhigh");
    assert_eq!(tried_tools, vec!["codex".to_string()]);
    assert_eq!(
        tried_specs,
        vec!["codex/openai/gpt-5.3-codex-spark/xhigh".to_string()]
    );
    assert_eq!(fallback_chain.len(), 1);
    assert!(!fallback_chain[0].quota_exhausted);
}

#[test]
fn codex_model_scoped_quota_result_tries_next_codex_model() {
    let _assume = assume_tools_available();
    let config = make_config(
        "tier-3-complex",
        &[
            "codex/openai/gpt-5.3-codex-spark/xhigh",
            "codex/openai/gpt-5.5/xhigh",
            "claude-code/anthropic/claude-sonnet/high",
        ],
    );
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_rate_limit_failover(
        "codex",
        &codex_spark_quota_result(),
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        true,
        Some("tier-3-complex"),
        None,
        None,
        true,
        "recover from codex rate limit",
        Path::new("."),
        Some(&config),
        None,
        Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
        &mut fallback_chain,
        None,
    )
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/gpt-5.5/xhigh");
    assert_eq!(fallback_chain.len(), 1);
    assert!(!fallback_chain[0].quota_exhausted);
}

#[test]
fn codex_model_scoped_quota_skips_unconfigured_openai_compat_fallback() {
    let _assume = assume_tools_available();
    let _base = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_BASE_URL");
    let _key = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_API_KEY");
    let _model = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_MODEL");
    let config = make_config(
        "tier-3-complex",
        &[
            "codex/openai/gpt-5.3-codex-spark/xhigh",
            "openai-compat/openai/gpt-5/high",
            "codex/openai/gpt-5.5/xhigh",
        ],
    );
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_error_rate_limit_failover(
        "codex",
        r#"Internal error: {"message":"You've hit your usage limit for GPT-5.3-Codex-Spark. Switch to another model now, or try again at Jun 11th, 2026 7:42 AM."}"#,
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        true,
        true,
        Some("tier-3-complex"),
        None,
        None,
        true,
        "recover from codex rate limit",
        Path::new("."),
        Some(&config),
        None,
        Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
        &mut fallback_chain,
        None,
    )
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/gpt-5.5/xhigh");
    assert!(tried_specs.contains(&"openai-compat/openai/gpt-5/high".to_string()));
    assert_eq!(fallback_chain.len(), 1);
}

#[test]
fn codex_model_scoped_quota_uses_globally_configured_openai_compat_fallback() {
    let _assume = assume_tools_available();
    let _base = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_BASE_URL");
    let _key = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_API_KEY");
    let _model = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_MODEL");
    let global_config = global_openai_compat_env_config();
    let config = make_config(
        "tier-3-complex",
        &[
            "codex/openai/gpt-5.3-codex-spark/xhigh",
            "openai-compat/openai/gpt-5/high",
            "codex/openai/gpt-5.5/xhigh",
        ],
    );
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_error_rate_limit_failover_with_global_config(
        "codex",
        r#"Internal error: {"message":"You've hit your usage limit for GPT-5.3-Codex-Spark. Switch to another model now, or try again at Jun 11th, 2026 7:42 AM."}"#,
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        true,
        true,
        Some("tier-3-complex"),
        None,
        None,
        true,
        "recover from codex rate limit",
        Path::new("."),
        Some(&config),
        Some(&global_config),
        None,
        Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
        &mut fallback_chain,
        None,
    )
    .expect("evaluate failover");

    assert_retry_to(action, "openai-compat", "openai-compat/openai/gpt-5/high");
    assert!(tried_specs.contains(&"codex/openai/gpt-5.3-codex-spark/xhigh".to_string()));
}

#[test]
fn codex_provider_quota_ignores_model_hint_quoted_outside_stderr() {
    let _assume = assume_tools_available();
    let config = make_config(
        "tier-3-complex",
        &[
            "codex/openai/gpt-5.3-codex-spark/xhigh",
            "codex/openai/gpt-5.5/xhigh",
            "claude-code/anthropic/claude-sonnet/high",
        ],
    );
    let exec_result = ExecutionResult {
        output: "Reviewed text includes: switch to another model.".to_string(),
        stderr_output: "You've hit your account usage limit. Try again later.".to_string(),
        summary: "Agent summary quotes: switch to another model.".to_string(),
        exit_code: 1,
        peak_memory_mb: None,
        ..Default::default()
    };
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_rate_limit_failover(
        "codex",
        &exec_result,
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        true,
        Some("tier-3-complex"),
        None,
        None,
        true,
        "review a prompt that mentions switching models",
        Path::new("."),
        Some(&config),
        None,
        Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
        &mut fallback_chain,
        None,
    )
    .expect("evaluate failover");

    assert_retry_to(
        action,
        "claude-code",
        "claude-code/anthropic/claude-sonnet/high",
    );
    assert_eq!(fallback_chain.len(), 1);
    assert!(fallback_chain[0].quota_exhausted);
}

#[test]
fn codex_model_scoped_quota_explains_when_no_fallback_candidate_exists() {
    let _assume = assume_tools_available();
    let config = make_config(
        "tier-3-complex",
        &["codex/openai/gpt-5.3-codex-spark/xhigh"],
    );
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_rate_limit_failover(
        "codex",
        &codex_spark_quota_result(),
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        true,
        Some("tier-3-complex"),
        None,
        None,
        true,
        "recover from codex rate limit",
        Path::new("."),
        Some(&config),
        None,
        Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
        &mut fallback_chain,
        None,
    )
    .expect("evaluate failover");

    match action {
        RateLimitAction::ExhaustedFailovers { reason } => {
            assert!(reason.contains("All tools in tier 'tier-3-complex'"));
        }
        RateLimitAction::Retry { .. } => panic!("expected exhausted failover, got retry"),
        RateLimitAction::NoRateLimit => panic!("expected exhausted failover, got no rate limit"),
    }
}
