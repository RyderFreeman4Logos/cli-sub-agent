use crate::run_cmd_post::{
    ErrorRateLimitFailoverRequest, RateLimitAction, RateLimitFailoverRequest,
    evaluate_error_rate_limit_failover, evaluate_error_rate_limit_failover_with_global_config,
    evaluate_rate_limit_failover,
};
use chrono::Utc;
use csa_config::global::GlobalToolConfig;
use csa_config::{GlobalConfig, ProjectConfig, ProjectMeta, TierConfig, TierStrategy};
use csa_core::types::ToolName;
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
        tool_state_dirs: HashMap::new(),
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

    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "codex",
        error_message: r#"Internal error: {"message":"You've hit your usage limit for GPT-5.3-Codex-Spark. Switch to another model now, or try again at Jun 11th, 2026 7:42 AM."}"#,
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: true,
        failover_on_crash_enabled: true,
        resolved_tier_name: Some("tier-3-complex"),
        tier_failover_tool_filter: Some(ToolName::Codex),
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "recover from codex rate limit",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: None,
    })
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
fn explicit_tool_in_tier_codex_quota_skips_unconfigured_openai_compat() {
    let _base = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_BASE_URL");
    let _key = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_API_KEY");
    let _model = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_MODEL");
    let config = make_config(
        "tier-3-complex",
        &[
            "codex/openai/gpt-5.3-codex-spark/xhigh",
            "openai-compat/openai/gpt-5/high",
        ],
    );
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "codex",
        error_message: r#"Internal error: {"message":"You've hit your usage limit for GPT-5.3-Codex-Spark. Switch to another model now, or try again at Jun 11th, 2026 7:42 AM."}"#,
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: true,
        failover_on_crash_enabled: true,
        resolved_tier_name: Some("tier-3-complex"),
        tier_failover_tool_filter: Some(ToolName::Codex),
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "recover from codex rate limit",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: None,
    })
    .expect("evaluate failover");

    match action {
        RateLimitAction::ExhaustedFailovers { reason } => {
            assert!(
                reason.contains("no executable codex fallback candidates"),
                "{reason}"
            );
            assert!(reason.contains("explicit --tool codex"), "{reason}");
        }
        RateLimitAction::Retry {
            new_tool,
            new_model_spec,
        } => panic!(
            "explicit codex tier run must not retry with {}/{}",
            new_tool.as_str(),
            new_model_spec.as_deref().unwrap_or("none")
        ),
        RateLimitAction::NoRateLimit => panic!("expected exhausted codex failover"),
    }
    assert!(tried_specs.contains(&"openai-compat/openai/gpt-5/high".to_string()));
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

    let action = evaluate_rate_limit_failover(RateLimitFailoverRequest {
        tool_name_str: "codex",
        exec_result: &codex_spark_quota_result(),
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: true,
        resolved_tier_name: Some("tier-3-complex"),
        tier_failover_tool_filter: None,
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "recover from codex rate limit",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: None,
    })
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

    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "codex",
        error_message: r#"Internal error: {"message":"You've hit your usage limit for GPT-5.3-Codex-Spark. Switch to another model now, or try again at Jun 11th, 2026 7:42 AM."}"#,
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: true,
        failover_on_crash_enabled: true,
        resolved_tier_name: Some("tier-3-complex"),
        tier_failover_tool_filter: None,
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "recover from codex rate limit",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: None,
    })
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
        ErrorRateLimitFailoverRequest {
            tool_name_str: "codex",
            error_message: r#"Internal error: {"message":"You've hit your usage limit for GPT-5.3-Codex-Spark. Switch to another model now, or try again at Jun 11th, 2026 7:42 AM."}"#,
            attempts: 1,
            max_failover_attempts: 4,
            tried_tools: &mut tried_tools,
            tried_specs: &mut tried_specs,
            tier_auto_select: true,
            failover_on_crash_enabled: true,
            resolved_tier_name: Some("tier-3-complex"),
            tier_failover_tool_filter: None,
            executed_session_id: None,
            effective_session_arg: None,
            ephemeral: true,
            prompt_text: "recover from codex rate limit",
            project_root: Path::new("."),
            config: Some(&config),
            global_config: Some(&global_config),
            task_needs_edit: None,
            current_model_spec: Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
            fallback_chain: &mut fallback_chain,
            attempt_elapsed: None,
        },
    )
    .expect("evaluate failover");

    assert_retry_to(action, "openai-compat", "openai-compat/openai/gpt-5/high");
    assert!(tried_specs.contains(&"codex/openai/gpt-5.3-codex-spark/xhigh".to_string()));
}

#[test]
fn explicit_tool_in_tier_codex_quota_skips_globally_configured_openai_compat() {
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
        ErrorRateLimitFailoverRequest {
            tool_name_str: "codex",
            error_message: r#"Internal error: {"message":"You've hit your usage limit for GPT-5.3-Codex-Spark. Switch to another model now, or try again at Jun 11th, 2026 7:42 AM."}"#,
            attempts: 1,
            max_failover_attempts: 4,
            tried_tools: &mut tried_tools,
            tried_specs: &mut tried_specs,
            tier_auto_select: true,
            failover_on_crash_enabled: true,
            resolved_tier_name: Some("tier-3-complex"),
            tier_failover_tool_filter: Some(ToolName::Codex),
            executed_session_id: None,
            effective_session_arg: None,
            ephemeral: true,
            prompt_text: "recover from codex rate limit",
            project_root: Path::new("."),
            config: Some(&config),
            global_config: Some(&global_config),
            task_needs_edit: None,
            current_model_spec: Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
            fallback_chain: &mut fallback_chain,
            attempt_elapsed: None,
        },
    )
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/gpt-5.5/xhigh");
    assert!(tried_specs.contains(&"openai-compat/openai/gpt-5/high".to_string()));
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

    let action = evaluate_rate_limit_failover(RateLimitFailoverRequest {
        tool_name_str: "codex",
        exec_result: &exec_result,
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: true,
        resolved_tier_name: Some("tier-3-complex"),
        tier_failover_tool_filter: None,
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "review a prompt that mentions switching models",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: None,
    })
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

    let action = evaluate_rate_limit_failover(RateLimitFailoverRequest {
        tool_name_str: "codex",
        exec_result: &codex_spark_quota_result(),
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: true,
        resolved_tier_name: Some("tier-3-complex"),
        tier_failover_tool_filter: None,
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "recover from codex rate limit",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: None,
    })
    .expect("evaluate failover");

    match action {
        RateLimitAction::ExhaustedFailovers { reason } => {
            assert!(reason.contains("All tools in tier 'tier-3-complex'"));
        }
        RateLimitAction::Retry { .. } => panic!("expected exhausted failover, got retry"),
        RateLimitAction::NoRateLimit => panic!("expected exhausted failover, got no rate limit"),
    }
}
