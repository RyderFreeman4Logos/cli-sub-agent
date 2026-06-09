use csa_session::Severity;

use super::{contains_blocking_review_signal, extract_review_findings_from_prose};

#[test]
fn issue_1971_blocking_regression_summary_is_blocking_signal() {
    assert!(contains_blocking_review_signal(
        "FAIL: one blocking test reliability regression found in the new provider-error failover coverage."
    ));
    assert!(!contains_blocking_review_signal(
        "Found one non-blocking test reliability regression."
    ));
}

#[test]
fn issue_1978_blocking_correctness_finding_summary_is_blocking_signal() {
    assert!(contains_blocking_review_signal(
        "One blocking correctness finding was found in csa review --session 01KTMDAQM18XK6R7DDA0ZP6C57 --fix tool selection."
    ));
    assert!(contains_blocking_review_signal(
        "Review found one blocking finding in the wait result classification."
    ));
    assert!(!contains_blocking_review_signal(
        "No blocking correctness findings were found in the review."
    ));
    assert!(!contains_blocking_review_signal(
        "No correctness, regression, security, or blocking test-coverage findings."
    ));
    assert!(contains_blocking_review_signal(
        "No prior context; one blocking finding remains."
    ));
}

#[test]
fn issue_1981_high_severity_summary_is_blocking_signal() {
    assert!(contains_blocking_review_signal(
        "Reviewed `main...HEAD` in read-only mode. Found 1 high-severity issue: `--memory-max-mb` can be accepted and used for admission projection."
    ));
    assert!(contains_blocking_review_signal(
        "Found one P1 correctness finding in the review output classifier."
    ));
}

#[test]
fn issue_1982_medium_correctness_remaining_summary_is_blocking_signal() {
    assert!(contains_blocking_review_signal(
        "One medium correctness finding remains after re-verifying the prior stale-FTS assumption. The rejudge-specific hard-delete path is fixed. FAIL"
    ));
}

#[test]
fn severe_summary_signal_respects_clean_negation() {
    assert!(!contains_blocking_review_signal(
        "PASS: no high or medium severity issues remain after review."
    ));
    assert!(!contains_blocking_review_signal(
        "No correctness, regression, security, or blocking test-coverage findings."
    ));
    assert!(!contains_blocking_review_signal(
        "Found one low-severity documentation issue."
    ));
}

#[test]
fn severity_metric_zero_count_bullets_do_not_become_prose_findings() {
    let findings = extract_review_findings_from_prose(
        r#"PASS: zero-count examples are clean.
- `High-severity: 0`
- `**High-severity**: 0`
- `Critical-severity: 0`
- `P1: 0`
"#,
    );

    assert!(
        findings.is_empty(),
        "zero-count severity metric examples must not become findings: {findings:?}"
    );
}

#[test]
fn severity_prefixed_bullet_with_description_still_becomes_prose_finding() {
    let findings = extract_review_findings_from_prose(
        "- High-severity: wait can still report success after a blocking review summary.",
    );

    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].severity, Severity::High);
}
