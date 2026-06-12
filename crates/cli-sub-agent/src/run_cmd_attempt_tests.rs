//! Tests for `run_cmd_attempt` helpers (extracted for monolith limit).

use crate::run_cmd::attempt_support::{
    allow_cross_tool_failover, build_failover_context_addendum,
    persist_fork_timeout_result_if_missing, resolve_attempt_initial_response_timeout_seconds,
};
use crate::run_cmd_model_pin::resolve_subtree_model_pin;
use crate::run_cmd_post::{
    ErrorRateLimitFailoverRequest, RateLimitAction, RateLimitFailoverRequest,
    evaluate_error_rate_limit_failover, evaluate_rate_limit_failover,
};
use crate::run_cmd_tool_selection::resolve_tool_by_strategy;
use crate::test_env_lock::ScopedTestEnvVar;
use crate::test_session_sandbox::ScopedSessionSandbox;
use anyhow::anyhow;
use chrono::Utc;
use csa_config::{GlobalConfig, ProjectConfig, ProjectMeta, TierConfig, TierStrategy};
use csa_core::env::CSA_MODEL_SPEC_ENV_KEY;
use csa_core::types::{ToolName, ToolSelectionStrategy};
use csa_process::ExecutionResult;
use csa_session::{create_session, load_result};
use std::{collections::HashMap, path::Path};

use super::resolve_attempt_subtree_model_pin_spec;

fn make_failover_config(models: &[&str]) -> ProjectConfig {
    make_named_failover_config("tier3", models)
}

fn make_named_failover_config(tier_name: &str, models: &[&str]) -> ProjectConfig {
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

fn assume_failover_tools_available() -> ScopedTestEnvVar {
    ScopedTestEnvVar::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1")
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

#[path = "run_cmd_attempt_fallback_chain_tests.rs"]
mod fallback_chain_tests;

#[test]
fn retry_subtree_pin_tracks_failover_attempt_model_spec() {
    let td = tempfile::tempdir().expect("tempdir");
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        "codex/openai/gpt-5.5/xhigh",
    ]);
    let global_config = GlobalConfig::default();
    let initial_pin = Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh");
    let failover_attempt_spec = Some("codex/openai/gpt-5.5/xhigh");

    let attempt_pin_spec =
        resolve_attempt_subtree_model_pin_spec(initial_pin, failover_attempt_spec);
    assert_eq!(attempt_pin_spec, failover_attempt_spec);

    let pin = resolve_subtree_model_pin(attempt_pin_spec, true, false)
        .expect("failover attempt should emit subtree pin");
    let entries: HashMap<&str, String> = pin.pin_env_entries().into_iter().collect();
    assert_eq!(
        entries.get(CSA_MODEL_SPEC_ENV_KEY).map(String::as_str),
        failover_attempt_spec
    );

    let child_finalizer = resolve_tool_by_strategy(
        &ToolSelectionStrategy::AnyAvailable,
        entries.get(CSA_MODEL_SPEC_ENV_KEY).map(String::as_str),
        None,
        None,
        Some(&config),
        &global_config,
        td.path(),
        false,
        false,
        false,
        None,
        true,
    )
    .expect("resolve child finalizer from failover attempt pin");

    assert_eq!(child_finalizer.tool, ToolName::Codex);
    assert_eq!(child_finalizer.model_spec.as_deref(), failover_attempt_spec);
}

#[test]
fn persist_fork_timeout_result_if_missing_skips_non_fork_sessions() {
    let td = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&td);
    let session =
        create_session(td.path(), Some("regular"), None, Some("codex")).expect("create session");

    persist_fork_timeout_result_if_missing(
        td.path(),
        false,
        ToolName::Codex,
        Some(&session.meta_session_id),
        chrono::Utc::now(),
        60,
    );

    assert!(
        load_result(td.path(), &session.meta_session_id)
            .expect("load result")
            .is_none(),
        "non-fork timeouts should not synthesize fork terminal results"
    );
}

#[test]
fn persist_fork_timeout_result_if_missing_writes_fork_failure() {
    let td = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&td);
    let parent = create_session(td.path(), Some("parent"), None, Some("codex")).expect("parent");
    let child = create_session(
        td.path(),
        Some("fork child"),
        Some(&parent.meta_session_id),
        Some("codex"),
    )
    .expect("child");

    persist_fork_timeout_result_if_missing(
        td.path(),
        true,
        ToolName::Codex,
        Some(&child.meta_session_id),
        chrono::Utc::now(),
        60,
    );

    let result = load_result(td.path(), &child.meta_session_id)
        .expect("load result")
        .expect("fork timeout result");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(
        result.summary.contains("wall-clock timeout"),
        "fork timeout result should explain the synthetic failure"
    );
}

#[test]
fn build_failover_context_addendum_includes_xurl_hint() {
    let addendum = build_failover_context_addendum("gemini-cli", Some("01ABCDEF"));
    assert!(addendum.is_some());
    let text = addendum.unwrap();
    assert!(text.contains("gemini-cli"), "should mention failed tool");
    assert!(text.contains("01ABCDEF"), "should mention session id");
    assert!(text.contains("csa xurl"), "should include xurl command");
    assert!(text.contains("gemini"), "should use gemini provider name");
}

#[test]
fn build_failover_context_addendum_none_without_session() {
    let addendum = build_failover_context_addendum("gemini-cli", None);
    assert!(addendum.is_none());
}

#[test]
fn build_failover_context_addendum_maps_claude_provider() {
    let addendum = build_failover_context_addendum("claude-code", Some("01XYZ"));
    assert!(addendum.is_some());
    let text = addendum.unwrap();
    assert!(
        text.contains("claude"),
        "should map claude-code to claude provider"
    );
}

#[test]
fn evaluate_error_rate_limit_failover_retries_on_acp_crash_retry_exhaustion() {
    let _assume = assume_failover_tools_available();
    let config = make_failover_config(&[
        "claude-code/anthropic/claude-sonnet/high",
        "codex/openai/o4-mini/high",
    ]);
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();

    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "claude-code",
        error_message: "ACP crash retry exhausted (2 attempts) for claude-code. Last error: server shut down unexpectedly",
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
        prompt_text: "investigate the crash",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("claude-code/anthropic/claude-sonnet/high"),
        fallback_chain: &mut vec![],
        attempt_elapsed: None,
    })
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/o4-mini/high");
}

#[test]
fn evaluate_error_rate_limit_failover_retries_on_gemini_retry_chain_exhaustion() {
    let _assume = assume_failover_tools_available();
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-2.5-pro/high",
        "codex/openai/o4-mini/high",
    ]);
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();

    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "gemini-cli",
        error_message: "Gemini ACP retry chain exhausted. OAuth->APIKey(same model)->APIKey(flash). Last error: temporary auth failure",
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
        prompt_text: "debug the request",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("gemini-cli/google/gemini-2.5-pro/high"),
        fallback_chain: &mut vec![],
        attempt_elapsed: None,
    })
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/o4-mini/high");
}

#[test]
fn issue_1730_evaluate_error_rate_limit_failover_retries_on_gemini_legacy_initial_stall() {
    let _assume = assume_failover_tools_available();
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-2.5-pro/high",
        "codex/openai/o4-mini/high",
    ]);
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();

    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "gemini-cli",
        error_message: "gemini_legacy_initial_stall: no stdout within 120s",
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
        prompt_text: "debug the stalled request",
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
    assert_eq!(fallback_chain.len(), 1);
    let attempt = &fallback_chain[0];
    assert_eq!(attempt.tool, "gemini-cli");
    assert_eq!(attempt.skip_reason, "gemini_legacy_initial_stall");
    assert!(!attempt.quota_exhausted);
}

#[test]
fn full_anyhow_chain_preserves_quota_markers_and_fails_over_to_next_provider() {
    // #1629: permanent quota exhaustion on gemini-cli (Google pool) MUST NOT
    // short-circuit failover; cross-provider alternatives (codex/OpenAI) still
    // count, and the gemini-cli quota attempt is recorded in fallback_chain.
    let _assume = assume_failover_tools_available();
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-2.5-pro/high",
        "codex/openai/o4-mini/high",
    ]);
    let error = anyhow!("daily quota exhausted for project")
        .context("ACP prompt failed")
        .context("Failed to execute tool via transport");

    assert_eq!(error.to_string(), "Failed to execute tool via transport");

    let rendered = format!("{error:#}");
    assert!(rendered.contains("Failed to execute tool via transport"));
    assert!(rendered.contains("ACP prompt failed"));
    assert!(rendered.contains("daily quota exhausted for project"));

    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();
    let mut fallback_chain = Vec::new();
    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "gemini-cli",
        error_message: &rendered,
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
        prompt_text: "debug the quota failure",
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
        "gemini-cli quota attempt must be recorded"
    );
    let entry = &fallback_chain[0];
    assert_eq!(entry.tool, "gemini-cli");
    assert!(entry.quota_exhausted, "entry must mark quota exhaustion");
}

#[test]
fn evaluate_error_rate_limit_failover_skips_retry_exhaustion_without_active_tier_routing() {
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-2.5-pro/high",
        "codex/openai/o4-mini/high",
    ]);
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();

    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "gemini-cli",
        error_message: "Gemini ACP retry chain exhausted. OAuth->APIKey(same model)->APIKey(flash). Last error: temporary auth failure",
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: false,
        failover_on_crash_enabled: false,
        resolved_tier_name: None,
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "debug the request",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("gemini-cli/google/gemini-2.5-pro/high"),
        fallback_chain: &mut vec![],
        attempt_elapsed: None,
    })
    .expect("evaluate failover");

    assert_no_rate_limit(action);
    assert!(
        tried_tools.is_empty(),
        "inactive tier routing should not mutate retry state"
    );
    assert!(
        tried_specs.is_empty(),
        "inactive tier routing should not mutate retry state"
    );
}

#[test]
fn evaluate_rate_limit_failover_skips_rate_limit_without_active_tier_routing() {
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-2.5-pro/high",
        "codex/openai/o4-mini/high",
    ]);
    let exec_result = ExecutionResult {
        output: String::new(),
        stderr_output: "429 resource exhausted".to_string(),
        summary: "rate limited".to_string(),
        exit_code: 1,
        peak_memory_mb: None,
        ..Default::default()
    };
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();

    let action = evaluate_rate_limit_failover(RateLimitFailoverRequest {
        tool_name_str: "gemini-cli",
        exec_result: &exec_result,
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: false,
        resolved_tier_name: None,
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "debug the request",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("gemini-cli/google/gemini-2.5-pro/high"),
        fallback_chain: &mut vec![],
        attempt_elapsed: None,
    })
    .expect("evaluate failover");

    assert_no_rate_limit(action);
    assert!(
        tried_tools.is_empty(),
        "inactive tier routing should not mutate retry state"
    );
    assert!(
        tried_specs.is_empty(),
        "inactive tier routing should not mutate retry state"
    );
}

#[test]
fn evaluate_rate_limit_failover_continues_past_permanent_quota_to_next_provider() {
    // #1629: gemini-cli (Google pool) RESOURCE_EXHAUSTED MUST fail over to
    // codex (OpenAI pool) instead of stopping the chain. Same-provider tools
    // would be skipped; cross-provider alternatives proceed.
    let _assume = assume_failover_tools_available();
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-2.5-pro/high",
        "codex/openai/o4-mini/high",
    ]);
    let exec_result = ExecutionResult {
        output: String::new(),
        stderr_output:
            "status RESOURCE_EXHAUSTED reason QUOTA_EXHAUSTED monthly spending cap reached"
                .to_string(),
        summary: "monthly spending cap reached".to_string(),
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
fn explicit_tool_in_tier_crash_triggers_failover() {
    let _assume = assume_failover_tools_available();
    let config = make_named_failover_config(
        "tier-3-complex",
        &[
            "codex/openai/o4-mini/high",
            "claude-code/anthropic/claude-sonnet/high",
        ],
    );
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();

    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "codex",
        error_message: "ACP crash retry exhausted (2 attempts) for codex. Last error: child process exited unexpectedly",
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: false,
        failover_on_crash_enabled: true,
        resolved_tier_name: Some("tier-3-complex"),
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "recover from codex crash",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("codex/openai/o4-mini/high"),
        fallback_chain: &mut vec![],
        attempt_elapsed: None,
    })
    .expect("evaluate failover");

    assert_retry_to(
        action,
        "claude-code",
        "claude-code/anthropic/claude-sonnet/high",
    );
}

#[test]
fn explicit_tool_no_tier_crash_no_failover() {
    let config = make_named_failover_config(
        "tier-3-complex",
        &[
            "codex/openai/o4-mini/high",
            "claude-code/anthropic/claude-sonnet/high",
        ],
    );
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();

    let action = evaluate_error_rate_limit_failover(ErrorRateLimitFailoverRequest {
        tool_name_str: "codex",
        error_message: "ACP crash retry exhausted (2 attempts) for codex. Last error: child process exited unexpectedly",
        attempts: 1,
        max_failover_attempts: 4,
        tried_tools: &mut tried_tools,
        tried_specs: &mut tried_specs,
        tier_auto_select: false,
        failover_on_crash_enabled: false,
        resolved_tier_name: None,
        executed_session_id: None,
        effective_session_arg: None,
        ephemeral: true,
        prompt_text: "recover from codex crash",
        project_root: Path::new("."),
        config: Some(&config),
        global_config: None,
        task_needs_edit: None,
        current_model_spec: Some("codex/openai/o4-mini/high"),
        fallback_chain: &mut vec![],
        attempt_elapsed: None,
    })
    .expect("evaluate failover");

    assert_no_rate_limit(action);
    assert!(
        tried_tools.is_empty(),
        "disabled crash failover should not mutate retry state"
    );
    assert!(
        tried_specs.is_empty(),
        "disabled crash failover should not mutate retry state"
    );
}

#[test]
fn explicit_tool_force_ignore_blocks_cross_tool_failover() {
    assert!(!allow_cross_tool_failover(
        ToolSelectionStrategy::Explicit(ToolName::Codex),
        None,
        true,
        false,
    ));
}

#[test]
fn explicit_tool_no_failover_blocks_cross_tool_failover() {
    assert!(!allow_cross_tool_failover(
        ToolSelectionStrategy::Explicit(ToolName::Codex),
        Some("tier-3-complex"),
        false,
        true,
    ));
}

// Regression #1440: explicit `--tool` blocks cross-tool failover even with `--tier`.
#[test]
fn explicit_tool_in_tier_blocks_cross_tool_failover() {
    let strategy = ToolSelectionStrategy::Explicit(ToolName::ClaudeCode);
    assert!(!allow_cross_tool_failover(
        strategy,
        Some("t4"),
        false,
        false
    ));
}

#[test]
fn resolve_attempt_initial_response_timeout_uses_fallback_tool_defaults() {
    let mut config = make_failover_config(&[
        "gemini-cli/google/gemini-2.5-pro/high",
        "codex/openai/o4-mini/high",
    ]);
    config.resources.initial_response_timeout_seconds = None;

    let gemini_timeout = resolve_attempt_initial_response_timeout_seconds(
        Some(&config),
        None,
        None,
        false,
        "gemini-cli",
    );
    let codex_timeout =
        resolve_attempt_initial_response_timeout_seconds(Some(&config), None, None, false, "codex");

    assert_eq!(
        gemini_timeout,
        Some(crate::pipeline::DEFAULT_GEMINI_INITIAL_RESPONSE_TIMEOUT_SECONDS)
    );
    assert_eq!(
        codex_timeout,
        Some(300),
        "runtime fallback to codex must use codex's default initial-response timeout"
    );
}

#[test]
fn resolve_attempt_initial_response_timeout_disables_codex_watchdog_for_ephemeral_runs() {
    let mut config = make_failover_config(&["codex/openai/o4-mini/high"]);
    config.resources.initial_response_timeout_seconds = Some(0);

    let codex_timeout =
        resolve_attempt_initial_response_timeout_seconds(Some(&config), None, None, false, "codex");

    assert_eq!(
        codex_timeout, None,
        "ephemeral codex runs must translate the disabled sentinel before execute_in"
    );
}
