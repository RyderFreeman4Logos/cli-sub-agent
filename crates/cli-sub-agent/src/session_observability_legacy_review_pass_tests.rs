use super::*;

#[test]
fn review_decision_value_accepts_clear_standalone_legacy_pass_decisions() {
    assert_eq!(
        review_decision_value("pass. Scope main...HEAD has no blocking issues"),
        Some(ReviewDecision::Pass)
    );
    assert_eq!(review_decision_value("CLEAN"), Some(ReviewDecision::Pass));
    assert_eq!(
        review_decision_value("**PASS** no findings"),
        Some(ReviewDecision::Pass)
    );
    assert_eq!(
        review_decision_value("PASS - all tests passed"),
        Some(ReviewDecision::Pass)
    );
    assert_eq!(
        review_decision_value("CLEAN/ no blocking findings"),
        Some(ReviewDecision::Pass)
    );
}

#[test]
fn legacy_plain_review_summary_decision_accepts_bounded_clean_verdict_shapes() {
    assert_eq!(
        legacy_plain_review_summary_decision("PASS\n"),
        Some(LegacyPlainReviewSummaryDecision::Decision(
            ReviewDecision::Pass
        ))
    );
    assert_eq!(
        legacy_plain_review_summary_decision("**CLEAN** — no blocking findings\n"),
        Some(LegacyPlainReviewSummaryDecision::Decision(
            ReviewDecision::Pass
        ))
    );
    assert_eq!(
        legacy_plain_review_summary_decision("Verdict: PASS - all tests passed\n"),
        Some(LegacyPlainReviewSummaryDecision::Decision(
            ReviewDecision::Pass
        ))
    );
    assert_eq!(
        legacy_plain_review_summary_decision("Status: CLEAN - all tests passed\n"),
        Some(LegacyPlainReviewSummaryDecision::Decision(
            ReviewDecision::Pass
        ))
    );
    assert_eq!(
        legacy_plain_review_summary_decision("Verdict: __PASS__ - all tests passed\n"),
        Some(LegacyPlainReviewSummaryDecision::Decision(
            ReviewDecision::Pass
        ))
    );
}

#[test]
fn legacy_plain_review_summary_decision_recognizes_bounded_parser_labels_fail_closed() {
    assert_eq!(
        legacy_plain_review_summary_decision("Status: FAIL\n"),
        Some(LegacyPlainReviewSummaryDecision::Decision(
            ReviewDecision::Fail
        ))
    );
    assert_eq!(
        legacy_plain_review_summary_decision("Result: clean-up required before merge\n"),
        Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel)
    );
    assert_eq!(
        legacy_plain_review_summary_decision("Status: pass/fail unclear\n"),
        Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel)
    );
    assert_eq!(
        legacy_plain_review_summary_decision(
            "Review: pass-through behavior still needs validation\n"
        ),
        Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel)
    );
}

#[test]
fn legacy_plain_review_summary_decision_rejects_ambiguous_label_after_bounded_pass() {
    assert_eq!(
        legacy_plain_review_summary_decision("PASS\nFinal verdict: pass/fail unclear\n"),
        Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel)
    );
    assert_eq!(
        legacy_plain_review_summary_decision("**CLEAN**\nFinal verdict: pass - fail unclear\n"),
        Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel)
    );
    assert_eq!(
        legacy_plain_review_summary_decision(
            "Status: PASS through behavior still needs validation\n"
        ),
        Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel)
    );
    assert_eq!(
        legacy_plain_review_summary_decision("Status: CLEAN up required before merge\n"),
        Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel)
    );
    assert_eq!(
        legacy_plain_review_summary_decision("Status: PASS - fail unclear\n"),
        Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel)
    );
}

#[test]
fn review_decision_value_rejects_compound_pass_like_values() {
    assert_eq!(
        review_decision_value("clean-up required before merge"),
        None
    );
    assert_eq!(
        review_decision_value("clean up required before merge"),
        None
    );
    assert_eq!(
        review_decision_value("pass-through behavior still needs validation"),
        None
    );
    assert_eq!(
        review_decision_value("pass through behavior still needs validation"),
        None
    );
    assert_eq!(review_decision_value("pass/fail unclear"), None);
    assert_eq!(review_decision_value("pass / fail unclear"), None);
    assert_eq!(review_decision_value("pass - fail unclear"), None);
    assert_eq!(review_decision_value("pass - clean up required"), None);
}

#[test]
fn legacy_plain_review_summary_decision_rejects_ambiguous_label_after_pass() {
    assert_eq!(
        legacy_plain_review_summary_decision(
            "Review result: pass\nFinal verdict: pass/fail unclear\n"
        ),
        Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel)
    );
    assert_eq!(
        legacy_plain_review_summary_decision(
            "Review result: pass\nFinal verdict: pass - fail unclear\n"
        ),
        Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel)
    );
}

#[test]
fn recovered_legacy_sidecar_decision_fails_closed_for_invalid_recognized_label() {
    assert_eq!(
        recovered_legacy_review_sidecar_decision(
            Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel),
            false,
        ),
        Some(ReviewDecision::Uncertain)
    );
}

#[test]
fn recovered_legacy_sidecar_decision_does_not_invent_no_decision_sidecar() {
    assert_eq!(
        legacy_plain_review_summary_decision(
            "Reviewer notes only: inspected the implementation and test plan.\n"
        ),
        None
    );
    assert_eq!(recovered_legacy_review_sidecar_decision(None, false), None);
}
