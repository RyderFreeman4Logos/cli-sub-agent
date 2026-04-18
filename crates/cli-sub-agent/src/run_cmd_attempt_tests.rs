//! Tests for `run_cmd_attempt` helpers (extracted for monolith limit).

use crate::run_cmd::attempt_support::{
    allow_cross_tool_failover, build_failover_context_addendum,
    persist_fork_timeout_result_if_missing, resolve_attempt_initial_response_timeout_seconds,
};
use crate::run_cmd_post::{RateLimitAction, evaluate_error_rate_limit_failover};
use crate::test_session_sandbox::ScopedSessionSandbox;
use anyhow::anyhow;
use chrono::Utc;
use csa_config::{ProjectConfig, ProjectMeta, TierConfig, TierStrategy};
use csa_core::types::{ToolName, ToolSelectionStrategy};
use csa_process::ExecutionResult;
use csa_session::{create_session, load_result};
use std::{collections::HashMap, path::Path};

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
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
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
        RateLimitAction::ExhaustedFailovers => {
            panic!("expected failover retry, got exhausted failovers")
        }
    }
}

fn assert_no_rate_limit(action: RateLimitAction) {
    match action {
        RateLimitAction::NoRateLimit => {}
        RateLimitAction::Retry { .. } => panic!("expected no failover, got retry"),
        RateLimitAction::ExhaustedFailovers => {
            panic!("expected no failover, got exhausted failovers")
        }
    }
}

#[test]
fn persist_fork_timeout_result_if_missing_skips_non_fork_sessions() {
    let td = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&td);
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
    let _sandbox = ScopedSessionSandbox::new(&td);
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
    let config = make_failover_config(&[
        "claude-code/anthropic/claude-sonnet/high",
        "codex/openai/o4-mini/high",
    ]);
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();

    let action = evaluate_error_rate_limit_failover(
        "claude-code",
        "ACP crash retry exhausted (2 attempts) for claude-code. Last error: server shut down unexpectedly",
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        true,
        true,
        Some("tier3"),
        None,
        None,
        true,
        "investigate the crash",
        Path::new("."),
        Some(&config),
        None,
        Some("claude-code/anthropic/claude-sonnet/high"),
    )
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/o4-mini/high");
}

#[test]
fn evaluate_error_rate_limit_failover_retries_on_gemini_retry_chain_exhaustion() {
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-2.5-pro/high",
        "codex/openai/o4-mini/high",
    ]);
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();

    let action = evaluate_error_rate_limit_failover(
        "gemini-cli",
        "Gemini ACP retry chain exhausted. OAuth->APIKey(same model)->APIKey(flash). Last error: temporary auth failure",
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        true,
        true,
        Some("tier3"),
        None,
        None,
        true,
        "debug the request",
        Path::new("."),
        Some(&config),
        None,
        Some("gemini-cli/google/gemini-2.5-pro/high"),
    )
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/o4-mini/high");
}

#[test]
fn full_anyhow_chain_preserves_quota_markers_for_failover_detection() {
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
    let action = evaluate_error_rate_limit_failover(
        "gemini-cli",
        &rendered,
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        true,
        true,
        Some("tier3"),
        None,
        None,
        true,
        "debug the quota failure",
        Path::new("."),
        Some(&config),
        None,
        Some("gemini-cli/google/gemini-2.5-pro/high"),
    )
    .expect("evaluate failover");

    assert_retry_to(action, "codex", "codex/openai/o4-mini/high");
}

#[test]
fn evaluate_error_rate_limit_failover_skips_retry_exhaustion_without_active_tier_routing() {
    let config = make_failover_config(&[
        "gemini-cli/google/gemini-2.5-pro/high",
        "codex/openai/o4-mini/high",
    ]);
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();

    let action = evaluate_error_rate_limit_failover(
        "gemini-cli",
        "Gemini ACP retry chain exhausted. OAuth->APIKey(same model)->APIKey(flash). Last error: temporary auth failure",
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        false,
        false,
        None,
        None,
        None,
        true,
        "debug the request",
        Path::new("."),
        Some(&config),
        None,
        Some("gemini-cli/google/gemini-2.5-pro/high"),
    )
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
    };
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();

    let action = crate::run_cmd_post::evaluate_rate_limit_failover(
        "gemini-cli",
        &exec_result,
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        false,
        None,
        None,
        None,
        true,
        "debug the request",
        Path::new("."),
        Some(&config),
        None,
        Some("gemini-cli/google/gemini-2.5-pro/high"),
    )
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
fn explicit_tool_in_tier_crash_triggers_failover() {
    let config = make_named_failover_config(
        "tier-3-complex",
        &[
            "codex/openai/o4-mini/high",
            "claude-code/anthropic/claude-sonnet/high",
        ],
    );
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();

    let action = evaluate_error_rate_limit_failover(
        "codex",
        "ACP crash retry exhausted (2 attempts) for codex. Last error: child process exited unexpectedly",
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        false,
        true,
        Some("tier-3-complex"),
        None,
        None,
        true,
        "recover from codex crash",
        Path::new("."),
        Some(&config),
        None,
        Some("codex/openai/o4-mini/high"),
    )
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

    let action = evaluate_error_rate_limit_failover(
        "codex",
        "ACP crash retry exhausted (2 attempts) for codex. Last error: child process exited unexpectedly",
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        false,
        false,
        None,
        None,
        None,
        true,
        "recover from codex crash",
        Path::new("."),
        Some(&config),
        None,
        Some("codex/openai/o4-mini/high"),
    )
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
fn explicit_tool_in_tier_ratelimit_no_failover() {
    let config = make_named_failover_config(
        "tier-3-complex",
        &[
            "codex/openai/o4-mini/high",
            "claude-code/anthropic/claude-sonnet/high",
        ],
    );
    let mut tried_tools = Vec::new();
    let mut tried_specs = Vec::new();

    let action = evaluate_error_rate_limit_failover(
        "codex",
        r#"Internal error: {"codex_error_info": "usage_limit_exceeded", "message": "You've hit your usage limit."}"#,
        1,
        4,
        &mut tried_tools,
        &mut tried_specs,
        false,
        true,
        Some("tier-3-complex"),
        None,
        None,
        true,
        "recover from codex rate limit",
        Path::new("."),
        Some(&config),
        None,
        Some("codex/openai/o4-mini/high"),
    )
    .expect("evaluate failover");

    assert_no_rate_limit(action);
    assert!(
        tried_tools.is_empty(),
        "rate limits should still respect tier auto-select state"
    );
    assert!(
        tried_specs.is_empty(),
        "rate limits should still respect tier auto-select state"
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

#[test]
fn explicit_tool_in_tier_keeps_cross_tool_failover_available() {
    assert!(allow_cross_tool_failover(
        ToolSelectionStrategy::Explicit(ToolName::Codex),
        Some("tier-3-complex"),
        false,
        false,
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

    assert_eq!(gemini_timeout, Some(120));
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
