//! Tests for `run_cmd_attempt` helpers (extracted for monolith limit).

use super::{build_failover_context_addendum, persist_fork_timeout_result_if_missing};
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_core::types::ToolName;
use csa_session::{create_session, load_result};

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
