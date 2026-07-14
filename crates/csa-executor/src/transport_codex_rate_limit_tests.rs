use super::*;
use csa_process::ExecutionResult;

#[test]
fn billing_code_content_not_false_positive() {
    assert!(!is_codex_permanent_quota_text(
        "implementing billing gate for enterprise"
    ));
    assert!(!is_codex_permanent_quota_text("fn process_billing_event()"));
    assert!(!is_codex_permanent_quota_text(
        "billing.rs: add monthly subscription handler"
    ));
}

#[test]
fn real_quota_errors_still_detected() {
    assert!(is_codex_permanent_quota_text("billing limit exceeded"));
    assert!(is_codex_permanent_quota_text("usage_limit_exceeded"));
    assert!(is_codex_permanent_quota_text("insufficient_quota"));
    assert!(is_codex_permanent_quota_text(
        "monthly spending cap reached"
    ));
}

#[test]
fn transient_numeric_status_requires_http_context() {
    assert!(!is_codex_transient_rate_limit_text(
        "/tmp/csa-worktree-lock-429-probe/session"
    ));
    assert!(is_codex_transient_rate_limit_text(
        "request failed with HTTP 429"
    ));
    assert!(is_codex_transient_rate_limit_text(
        "provider returned statusCode: 429"
    ));
}

#[test]
fn stdout_billing_content_not_false_positive_in_result() {
    let execution = ExecutionResult {
        exit_code: 1,
        summary: String::new(),
        stderr_output: "connection timeout".to_string(),
        output: "implementing billing gate for enterprise".to_string(),
        ..Default::default()
    };

    assert!(!is_codex_permanent_quota_result(&execution));
}
