use crate::run_cmd_post::{
    ErrorRateLimitFailoverRequest, RateLimitAction, RateLimitFailoverRequest,
    evaluate_error_rate_limit_failover, evaluate_rate_limit_failover,
};
use chrono::Utc;
use csa_config::{ProjectConfig, ProjectMeta, TierConfig, TierStrategy};
use csa_process::ExecutionResult;
use std::{collections::HashMap, path::Path, time::Duration};

fn make_failover_config(models: &[&str]) -> ProjectConfig {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier3".to_string(),
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
        tier_mapping: HashMap::from([("default".to_string(), "tier3".to_string())]),
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

fn assert_no_rate_limit(action: RateLimitAction) {
    match action {
        RateLimitAction::NoRateLimit => {}
        RateLimitAction::Retry { .. } => panic!("expected no failover, got retry"),
        RateLimitAction::ExhaustedFailovers { .. } => {
            panic!("expected no failover, got exhausted failovers")
        }
    }
}

fn assume_tools_available() -> crate::test_env_lock::ScopedTestEnvVar {
    crate::test_env_lock::ScopedTestEnvVar::set(
        crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV,
        "1",
    )
}

#[test]
fn evaluate_error_rate_limit_failover_retries_on_gemini_http_400_init_failure() {
    let _assume = assume_tools_available();
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        "codex/openai/gpt-5.4/high",
    ]);
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "gemini-cli",
        error_message: "Gemini request failed: status: 400 Bad Request",
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: true,
        failover_on_crash_enabled: true,
        resolved_tier_name: Some("tier3"),
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "do some work",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: Some(Duration::from_secs(2)),
    })
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/gpt-5.4/high");
    assert_eq!(fallback_chain.len(), 1);
    assert_eq!(fallback_chain[0].skip_reason, "status 400");
}

#[test]
fn evaluate_error_rate_limit_failover_skips_http_400_after_init_window() {
    let _assume = assume_tools_available();
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        "codex/openai/gpt-5.4/high",
    ]);
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "gemini-cli",
        error_message: "Gemini request failed after work started: status: 400 Bad Request",
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: true,
        failover_on_crash_enabled: true,
        resolved_tier_name: Some("tier3"),
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "do some work",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: Some(Duration::from_secs(31)),
    })
    .expect("evaluate failover");

    assert_no_rate_limit(action);
    assert!(tried_tools.is_empty());
    assert!(tried_specs.is_empty());
    assert!(fallback_chain.is_empty());
}

#[test]
fn evaluate_rate_limit_failover_retries_on_http_500_init_result() {
    let _assume = assume_tools_available();
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        "codex/openai/gpt-5.4/high",
    ]);
    let exec_result = ExecutionResult {
        output: String::new(),
        stderr_output: "HTTP 500 Internal Server Error".to_string(),
        summary: "HTTP 500 Internal Server Error".to_string(),
        exit_code: 1,
        peak_memory_mb: None,
        ..Default::default()
    };
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_rate_limit_failover(RateLimitFailoverRequest {
        tool_name_str: "gemini-cli",
        exec_result: &exec_result,
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: true,
        resolved_tier_name: Some("tier3"),
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "debug the request",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: Some(Duration::from_secs(2)),
    })
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/gpt-5.4/high");
    assert_eq!(fallback_chain.len(), 1);
    assert_eq!(fallback_chain[0].skip_reason, "http 500");
}

#[test]
fn evaluate_rate_limit_failover_skips_http_500_after_init_window() {
    let _assume = assume_tools_available();
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        "codex/openai/gpt-5.4/high",
    ]);
    let exec_result = ExecutionResult {
        output: "meaningful work already happened".to_string(),
        stderr_output: "HTTP 500 Internal Server Error".to_string(),
        summary: "HTTP 500 Internal Server Error".to_string(),
        exit_code: 1,
        peak_memory_mb: None,
        ..Default::default()
    };
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_rate_limit_failover(RateLimitFailoverRequest {
        tool_name_str: "gemini-cli",
        exec_result: &exec_result,
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: true,
        resolved_tier_name: Some("tier3"),
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "debug the request",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: Some(Duration::from_secs(31)),
    })
    .expect("evaluate failover");

    assert_no_rate_limit(action);
    assert!(tried_tools.is_empty());
    assert!(tried_specs.is_empty());
    assert!(fallback_chain.is_empty());
}
