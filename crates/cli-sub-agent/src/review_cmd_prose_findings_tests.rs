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

#[test]
fn issue_2516_resolved_high_finding_prose_is_not_blocking_signal() {
    assert!(!contains_blocking_review_signal(
        "The prior high finding is fixed and no blocking findings remain."
    ));
    assert!(contains_blocking_review_signal(
        "One high severity finding remains in the review output parser."
    ));
}

#[test]
fn issue_2516_positive_verification_paths_do_not_become_medium_findings() {
    let findings = extract_review_findings_from_prose(
        r#"PASS: no blocking findings remain.
Verification:
- crates/cli-sub-agent/src/review_cmd_output.rs:139 confirms the prior high finding is fixed.
- tests/review.rs:42 verifies the remediation path.
"#,
    );

    assert!(
        findings.is_empty(),
        "positive verification path bullets must not become findings: {findings:?}"
    );
}

#[test]
fn issue_2601_chinese_positive_evidence_bullets_do_not_become_prose_findings() {
    let findings = extract_review_findings_from_prose(concat!(
        "PASS\n",
        "- P1/P2/C1: \u{9ed8}\u{8ba4} evidence \u{5173}\u{95ed}\u{3001}raw ",
        "\u{5173}\u{95ed}\u{3001}XDG \u{9ed8}\u{8ba4}\u{8def}\u{5f84}",
        "\u{4e0e}\u{8def}\u{5f84}\u{8986}\u{76d6}/\u{975e}\u{6cd5}",
        "\u{8def}\u{5f84}\u{6821}\u{9a8c}\u{5728} settings \u{4e2d}",
        "\u{5df2}\u{5b9e}\u{73b0}\u{5e76}\u{6d4b}\u{8bd5}\n",
        "- P2: CLI \u{900f}\u{4f20}\u{5df2}\u{6709}\u{76f4}\u{63a5}",
        "\u{6d4b}\u{8bd5}\n",
        "- C1: XDG override \u{5df2}\u{901a}\u{8fc7}\u{8def}\u{5f84}",
        "\u{6821}\u{9a8c}\u{7f13}\u{89e3}\n",
        "- P1: fallback \u{884c}\u{4e3a}\u{5df2}\u{6709}\u{673a}\u{68b0}",
        "\u{6d4b}\u{8bd5}\n",
        "- P2: reviewer summary \u{5df2}\u{8986}\u{76d6}\n",
    ));

    assert!(
        findings.is_empty(),
        "Chinese positive-evidence bullets must not become findings: {findings:?}"
    );
}

#[test]
fn issue_2440_verified_word_does_not_trigger_resolution() {
    use super::finding_text_describes_resolved_issue;

    assert!(
        !finding_text_describes_resolved_issue(
            "High: verified reviewer-auth credential disclosure in summary"
        ),
        "'verified' in an active finding title is not explicit resolution language"
    );
}

#[test]
fn issue_2516_no_longer_describes_regression_not_resolution() {
    use super::finding_text_describes_resolved_issue;

    assert!(
        !finding_text_describes_resolved_issue("crates/foo.rs:42 no longer validates user input"),
        "'no longer validates' is a regression description, not a resolution"
    );
    assert!(
        !finding_text_describes_resolved_issue("crates/foo.rs:42 no longer checks for overflow"),
        "'no longer checks' is a regression description, not a resolution"
    );
    assert!(
        finding_text_describes_resolved_issue(
            "The prior high finding is addressed and no longer present"
        ),
        "'no longer present' after a resolution word should be classified as resolved"
    );
}

#[test]
fn issue_2601_chinese_resolution_phrases_describe_resolved_issue() {
    use super::finding_text_describes_resolved_issue;

    for text in [
        concat!(
            "P1/P2/C1\u{ff1a}\u{9ed8}\u{8ba4} evidence \u{5173}\u{95ed}\u{3001}raw ",
            "\u{5173}\u{95ed}\u{3001}XDG \u{9ed8}\u{8ba4}\u{8def}\u{5f84}",
            "\u{4e0e}\u{8def}\u{5f84}\u{8986}\u{76d6}/\u{975e}\u{6cd5}",
            "\u{8def}\u{5f84}\u{6821}\u{9a8c}\u{5df2}\u{5b9e}\u{73b0}",
            "\u{5e76}\u{6d4b}\u{8bd5}",
        ),
        "P2\u{ff1a}CLI \u{8f93}\u{51fa}\u{5df2}\u{6709}\u{76f4}\u{63a5}\u{6d4b}\u{8bd5}",
        "C1\u{ff1a}\u{975e}\u{6cd5}\u{8def}\u{5f84}\u{5df2}\u{901a}\u{8fc7} settings \u{6821}\u{9a8c}\u{7f13}\u{89e3}",
        "P1\u{ff1a}\u{8986}\u{76d6}\u{7387}\u{5df2}\u{6709}\u{673a}\u{68b0}\u{6d4b}\u{8bd5}",
        "P2\u{ff1a}\u{72b6}\u{6001}\u{6458}\u{8981}\u{5df2}\u{8986}\u{76d6}",
    ] {
        assert!(
            finding_text_describes_resolved_issue(text),
            "Chinese positive evidence should be classified as resolved: {text}"
        );
    }
}

#[test]
fn issue_2601_chinese_active_problem_phrases_stay_blocking() {
    use super::finding_text_describes_resolved_issue;

    assert!(
        !finding_text_describes_resolved_issue(
            "P1: settings \u{8def}\u{5f84}\u{6821}\u{9a8c}\u{672a}\u{8986}\u{76d6}"
        ),
        "Chinese active problem prose must not be classified as resolved"
    );
    assert!(
        !finding_text_describes_resolved_issue(
            "P1: evidence \u{5df2}\u{8986}\u{76d6}\u{4e0d}\u{8db3}"
        ),
        "Chinese insufficiency prose must not be classified as resolved"
    );
}

#[test]
fn issue_2516_mixed_resolution_and_active_problem_stays_blocking() {
    use super::review_signal_describes_resolved_issue;

    let tokens: Vec<String> = "fixed high finding still remains"
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let signal_index = tokens
        .iter()
        .position(|t| t == "fixed")
        .expect("find signal token");
    let noun_index = tokens
        .iter()
        .position(|t| t == "finding")
        .expect("find noun token");
    assert!(
        !review_signal_describes_resolved_issue(&tokens, signal_index, noun_index),
        "sentence with 'still remains' must not be classified as resolved"
    );
}

#[test]
fn issue_2637_compound_priority_spec_not_extracted_as_finding() {
    // "P1/P2/P3: code paths now classify..." is acceptance-criteria prose,
    // not a finding. It must not be extracted as a HIGH finding.
    let text = "## Findings\nP1/P2/P3: code paths now classify status, connect, timeout into bounded cause labels.\n";
    let findings = extract_review_findings_from_prose(text);
    assert!(
        findings.is_empty(),
        "compound P1/P2/P3 spec should not be extracted as a finding"
    );
}

#[test]
fn issue_2637_single_priority_with_colon_still_parses() {
    // Single "P1:" should still be parsed as HIGH severity.
    let text = "## Findings\nP1: critical race in lock acquisition path\n";
    let findings = extract_review_findings_from_prose(text);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].severity, Severity::High);
}

#[test]
fn issue_2637_compound_priority_not_blocking_signal() {
    // Compound spec should not trigger blocking signal extraction either.
    assert!(!contains_blocking_review_signal(
        "P1/P2/P3: code paths now classify status into bounded cause labels."
    ));
}
