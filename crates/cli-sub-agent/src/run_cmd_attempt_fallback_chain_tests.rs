//! Fallback-chain and quota-pool tests for `run_cmd_attempt`.

use crate::run_cmd_post::{
    ErrorRateLimitFailoverRequest, RateLimitAction, RateLimitFailoverRequest,
    evaluate_error_rate_limit_failover, evaluate_rate_limit_failover,
};
use chrono::Utc;
use csa_process::ExecutionResult;
use std::path::Path;

use super::{
    assert_no_rate_limit, assert_retry_to, make_failover_config, make_named_failover_config,
};

// --- fallback_chain population tests (#1346) ---

#[test]
fn evaluate_error_rate_limit_failover_populates_fallback_chain_on_transient_retry_chain() {
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-2.5-pro/high",
        "codex/openai/o4-mini/high",
    ]);
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "gemini-cli",
        error_message: "Gemini ACP retry chain exhausted. OAuth->APIKey(same model)->APIKey(flash). Last error: resource exhausted",
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
        current_model_spec: Some("gemini-cli/google/gemini-2.5-pro/high"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: None,
    })
    .expect("evaluate failover");

    assert!(
        matches!(action, RateLimitAction::Retry { .. }),
        "expected retry action"
    );
    assert_eq!(fallback_chain.len(), 1, "one attempt should be recorded");
    let attempt = &fallback_chain[0];
    assert_eq!(attempt.tool, "gemini-cli");
    assert_eq!(
        attempt.model_spec.as_deref(),
        Some("gemini-cli/google/gemini-2.5-pro/high")
    );
    assert!(
        !attempt.quota_exhausted,
        "retry chain exhaustion is not permanent quota exhaustion by itself"
    );
}

#[test]
fn evaluate_error_rate_limit_failover_continues_past_permanent_gemini_quota() {
    // #1629: gemini-cli permanent quota (via retry chain exhaustion with
    // QUOTA_EXHAUSTED tail) MUST fail over to codex (different provider).
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-2.5-pro/high",
        "codex/openai/o4-mini/high",
    ]);
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "gemini-cli",
        error_message: "Gemini ACP retry chain exhausted. Last error: status RESOURCE_EXHAUSTED reason QUOTA_EXHAUSTED monthly spending cap reached",
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
        current_model_spec: Some("gemini-cli/google/gemini-2.5-pro/high"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: None,
    })
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/o4-mini/high");
    assert_eq!(tried_tools, vec!["gemini-cli".to_string()]);
    assert_eq!(
        tried_specs,
        vec!["gemini-cli/google/gemini-2.5-pro/high".to_string()]
    );
    assert_eq!(
        fallback_chain.len(),
        1,
        "gemini-cli quota attempt must be recorded for audit"
    );
    let entry = &fallback_chain[0];
    assert_eq!(entry.tool, "gemini-cli");
    assert!(entry.quota_exhausted, "entry must mark quota exhaustion");
}

#[test]
fn evaluate_error_rate_limit_failover_no_failover_flag_prevents_fallback_chain_entry() {
    let config = make_named_failover_config(
        "tier-3-complex",
        &[
            "gemini-cli/google/gemini-2.5-pro/high",
            "codex/openai/o4-mini/high",
        ],
    );
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    // tier_auto_select=false suppresses rate-limit failover entirely (#1346);
    // the caller observes NoRateLimit and no fallback_chain entry is recorded.
    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "gemini-cli",
        error_message: "quota_exhausted for this account",
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: false, // no failover
        failover_on_crash_enabled: true,
        resolved_tier_name: Some("tier-3-complex"),
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "do some work",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("gemini-cli/google/gemini-2.5-pro/high"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: None,
    })
    .expect("evaluate failover");

    assert_no_rate_limit(action);
    assert!(
        fallback_chain.is_empty(),
        "no fallback_chain entry when failover is suppressed (#1346)"
    );
}

// --- Same-provider quota-pool skip tests (#1629) ---

#[test]
fn evaluate_rate_limit_failover_gemini_quota_skips_antigravity_picks_codex() {
    // tier-4-critical-style ordering: gemini-cli -> antigravity-cli -> codex.
    // Both gemini-cli and antigravity-cli share Google's quota pool, so once
    // gemini-cli is marked permanently exhausted, antigravity-cli MUST be
    // skipped and codex (OpenAI) selected.
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        "antigravity-cli/google/gemini-3.1-pro/high",
        "codex/openai/gpt-5.5/high",
        "claude-code/anthropic/claude-sonnet-4-7/high",
    ]);
    let exec_result = ExecutionResult {
        output: String::new(),
        stderr_output:
            "status RESOURCE_EXHAUSTED reason QUOTA_EXHAUSTED monthly spending cap reached"
                .to_string(),
        summary: "daily quota for project exceeded".to_string(),
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
        attempt_elapsed: None,
    })
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/gpt-5.5/high");
}

#[test]
fn evaluate_rate_limit_failover_chained_google_pool_exhaustion_continues_past() {
    // Simulate the second pass: gemini-cli was already attempted and recorded
    // with quota_exhausted=true in fallback_chain. Now antigravity-cli (same
    // Google pool) also hits quota. The decision must skip antigravity-cli
    // (same provider) and pick codex.
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        "antigravity-cli/google/gemini-3.1-pro/high",
        "codex/openai/gpt-5.5/high",
    ]);
    let exec_result = ExecutionResult {
        output: String::new(),
        stderr_output:
            "status RESOURCE_EXHAUSTED reason QUOTA_EXHAUSTED monthly spending cap reached"
                .to_string(),
        summary: "Google quota pool exhausted".to_string(),
        exit_code: 1,
        peak_memory_mb: None,
        ..Default::default()
    };
    let mut tried_tools = vec!["gemini-cli".to_string()];
    let mut tried_specs = vec!["gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string()];
    let mut fallback_chain = vec![csa_core::types::FallbackAttempt {
        tool: "gemini-cli".to_string(),
        model_spec: Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string()),
        skip_reason: "RESOURCE_EXHAUSTED".to_string(),
        quota_exhausted: true,
        timestamp: Utc::now(),
    }];

    let action = evaluate_rate_limit_failover(RateLimitFailoverRequest {
        tool_name_str: "antigravity-cli",
        exec_result: &exec_result,
        attempts: 2,
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
        current_model_spec: Some("antigravity-cli/google/gemini-3.1-pro/high"),
        fallback_chain: &mut fallback_chain,
        attempt_elapsed: None,
    })
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/gpt-5.5/high");
    assert_eq!(
        fallback_chain.len(),
        2,
        "both gemini-cli and antigravity-cli quota attempts must be recorded"
    );
    let antigravity_entry = &fallback_chain[1];
    assert_eq!(antigravity_entry.tool, "antigravity-cli");
    assert!(
        antigravity_entry.quota_exhausted,
        "antigravity-cli entry must mark quota exhaustion"
    );
}
