//! Tests for `run_cmd_attempt` helpers (extracted for monolith limit).

use super::{build_failover_context_addendum, persist_fork_timeout_result_if_missing};
use crate::run_cmd_post::{RateLimitAction, evaluate_error_rate_limit_failover};
use crate::test_session_sandbox::ScopedSessionSandbox;
use anyhow::anyhow;
use chrono::Utc;
use csa_config::{ProjectConfig, ProjectMeta, TierConfig, TierStrategy};
use csa_core::types::ToolName;
use csa_session::{create_session, load_result};
use std::{collections::HashMap, path::Path};

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
