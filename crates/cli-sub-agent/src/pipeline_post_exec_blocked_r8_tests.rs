//! R8 root-cause regression tests for the blocked-output classifier.
//!
//! Covers two root-cause defect classes that case-by-case patches kept
//! reintroducing (#2806 convergence protocol, rule 076):
//!
//! 1. **Boundary scanning**: extracted text (content-block arrays, multi-line
//!    tool output) is scanned as a single string with start-anchored checks,
//!    so a hard marker on a trailing line/element is missed.
//! 2. **Forgeable pass signal**: `gate_bound_pass_signal` matches pass tokens
//!    embedded mid-sentence in free-form prose, suppressing a real unresolved
//!    gate diagnostic.

use super::{
    line_reports_unresolved_gate_outcome, message_reports_gate_resolution,
    worker_output_indicates_blocked, worker_output_indicates_blocked_with_receipt,
};

// --- Boundary scanning: content-array / multi-line markers ---

#[test]
fn r8_content_array_trailing_blocked_marker_is_detected() {
    // R8-F1: a STATUS: BLOCKED marker in the SECOND content-block element must
    // not be lost when content blocks are joined.
    let envelope = r#"{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"running gate"},{"type":"text","text":"STATUS: BLOCKED — tool failed"}]}"#;
    assert!(
        worker_output_indicates_blocked_with_receipt(envelope, "", "retry ok", true),
        "trailing content-block STATUS: BLOCKED must block: {envelope}"
    );
}

#[test]
fn r8_content_array_trailing_unconfirmed_gate_marker_is_detected() {
    // R8-F1: an unconfirmed-gate marker in a later content-block element.
    let envelope = r#"{"type":"tool_result","content":[{"type":"text","text":"preflight ok"},{"type":"text","text":"zsh: status unknown"}]}"#;
    assert!(
        worker_output_indicates_blocked_with_receipt(envelope, "", "retry ok", true),
        "trailing content-block unconfirmed-gate marker must block: {envelope}"
    );
}

#[test]
fn r8_assistant_content_array_trailing_blocked_marker_is_detected() {
    // R8-F1: Claude assistant content blocks joined without a separator must
    // not hide a trailing hard marker in the agent-message suppression path.
    let envelope = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"retried"},{"type":"text","text":"STATUS: BLOCKED"}]}}"#;
    assert!(
        worker_output_indicates_blocked_with_receipt(envelope, "", "retry ok", true),
        "assistant content-array trailing STATUS: BLOCKED must block: {envelope}"
    );
}

#[test]
fn r8_multiline_tool_output_trailing_blocked_is_detected() {
    // R8-F1: a tool_result string value that itself contains newlines must be
    // scanned per-line, not only at its start.
    let envelope =
        "{\"type\":\"tool_result\",\"content\":\"line one\\nSTATUS: BLOCKED\\nline three\"}";
    assert!(
        worker_output_indicates_blocked_with_receipt(envelope, "", "retry ok", true),
        "multi-line tool_result trailing STATUS: BLOCKED must block: {envelope}"
    );
}

#[test]
fn r8_multiline_tool_output_trailing_unconfirmed_gate_is_detected() {
    // R8-F1: trailing unconfirmed-gate marker inside a multi-line string value.
    let envelope = "{\"type\":\"tool_result\",\"content\":\"ok\\nbash: exit status unknown\"}";
    assert!(
        worker_output_indicates_blocked_with_receipt(envelope, "", "retry ok", true),
        "multi-line tool_result trailing unconfirmed-gate must block: {envelope}"
    );
}

#[test]
fn r8_summary_trailing_blocked_after_newline_is_detected() {
    // R8-F1: the summary path must also scan every line, not only the start.
    assert!(worker_output_indicates_blocked(
        "",
        "retried the gate\nSTATUS: BLOCKED — still failing"
    ));
    assert!(worker_output_indicates_blocked(
        "",
        "preflight\nzsh: status unknown"
    ));
}

// --- Forgeable gate-bound pass signal ---

#[test]
fn r8_free_form_prose_does_not_forge_gate_pass() {
    // R8-F2: "passed the gate" embedded mid-sentence must not resolve a gate.
    for message in [
        "I passed the gate logs to the maintainer",
        "I passed the gate report to the maintainer",
        "then passed the gate output downstream for review",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "mid-sentence 'passed the gate' must not resolve: {message}"
        );
        let agent_message = format!(r#"{{"agent_message":"Gate status unknown; {message}"}}"#);
        assert!(
            worker_output_indicates_blocked_with_receipt(&agent_message, "", "retry ok", true),
            "current receipt + forged pass prose must still block: {agent_message}"
        );
    }
}

#[test]
fn r8_concurrent_unknown_vetoes_pass_signal() {
    // R8-F2: a concurrent "gate status unknown/unavailable/lost" must
    // unconditionally veto a pass claim, even when the pass signal is
    // syntactically gate-bound.
    for message in [
        "gate passed, but gate status is unknown",
        "gate status: passed, however gate status unavailable",
        "result: pass, yet gate status lost",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "concurrent unknown/unavailable/lost must veto pass: {message}"
        );
    }
}

#[test]
fn r8_legitimate_gate_bound_resolution_still_resolves() {
    // Non-regression: genuine fully-anchored resolution phrases still resolve.
    for message in [
        "gate passed",
        "gate status: passed",
        "result: pass",
        "reran the gate and it now passes the gate",
        "prior gate status unknown; gate passed after retry",
        "previous gate status unknown; gate status: passed",
        "prior gate status unknown; result: pass on retry",
    ] {
        assert!(
            message_reports_gate_resolution(message),
            "legitimate gate-bound resolution must resolve: {message}"
        );
    }
}

#[test]
fn r8_prior_resolution_classes_still_hold() {
    // Non-regression for R4-R7 fixed classes.
    assert!(message_reports_gate_resolution(
        "Initial gate status was unknown; reran the gate and it now PASSes the gate."
    ));
    assert!(!message_reports_gate_resolution(
        "gate passed, but tests and commit omitted"
    ));
    // Bare "failed" must remain benign for require-commit rescue.
    assert!(!line_reports_unresolved_gate_outcome(
        "writer completed but commit failed"
    ));
}

// --- R10-F1: full-trailing-token gate-signal anchoring ---

#[test]
fn r10_late_gate_object_noun_after_bare_gate_passed_is_rejected() {
    // R10-F1: a gate-object noun ANYWHERE in the trailing tokens proves the
    // clause is mid-sentence prose. The adjacent-only check missed this.
    for message in [
        "the parser gate passed sanitized data downstream",
        "the parser gate passed data downstream",
        "Gate status unknown earlier; the parser gate passed sanitized data downstream",
        "the gate passed output to the maintainer for review",
        "the status passed logs downstream",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "late gate-object noun must not forge a pass: {message}"
        );
    }
}

#[test]
fn r10_gate_status_pass_with_trailing_gate_object_is_rejected() {
    // R10-F1: the "gate status pass" / "status is pass" branches (which
    // previously skipped the anchor check entirely) must now reject a trailing
    // gate-object noun.
    for message in [
        "status is pass data downstream",
        "result is pass output to the reviewer",
        "gate status pass logs downstream",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "'status is pass' / 'gate status pass' with trailing gate object must not forge: {message}"
        );
    }
}

#[test]
fn r10_legitimate_resolutions_survive_full_token_scan() {
    // Non-regression: genuine resolutions whose trailing tokens are NOT
    // gate-object nouns must still resolve after the full-token scan.
    for message in [
        "gate passed",
        "gate passed after retry",
        "gate status: passed",
        "result: pass on retry",
        "status is pass",
        "gate status pass",
        "prior gate status unknown; gate passed after retry",
        "reran the gate and it now passes the gate",
    ] {
        assert!(
            message_reports_gate_resolution(message),
            "legitimate resolution must still resolve: {message}"
        );
    }
}

// --- R10-F2: line-scoped historical omitted exemption ---

#[test]
fn r10_mixed_historical_and_current_omission_still_flags_current() {
    // R10-F2: a mixed message with one historical-omission line and one
    // CURRENT-omission line must still flag the current omission. The whole-text
    // exemption previously suppressed both.
    let mixed = "the previous turn omitted tests and commit\n\
                 tests and commit omitted this turn";
    assert!(
        !message_reports_gate_resolution(mixed),
        "mixed historical + current omission must not resolve: {mixed}"
    );
    // The mixed message must still be flagged as blocked even with a receipt.
    assert!(
        worker_output_indicates_blocked_with_receipt(mixed, "", "retry ok", true),
        "current receipt + mixed omission must still block: {mixed}"
    );
}

#[test]
fn r10_pure_historical_omission_does_not_veto_gate_pass() {
    // Non-regression: a message whose ONLY omission line carries the historical
    // marker must NOT veto a genuine gate-bound pass (line-scoped exemption).
    let historical = "the previous turn omitted tests and commit; this turn completed both";
    assert!(
        message_reports_gate_resolution(&format!("gate passed. {historical}")),
        "gate passed + pure historical omission must resolve"
    );
    // Without a pass signal the historical line alone does not resolve — but it
    // must not be flagged as a current omission either.
    assert!(
        !message_reports_gate_resolution(historical),
        "historical narration without a pass signal does not resolve (no positive signal)"
    );
    assert!(
        !worker_output_indicates_blocked_with_receipt(historical, "", "retry ok", true),
        "pure historical omission must not block even as a summary: {historical}"
    );
}
