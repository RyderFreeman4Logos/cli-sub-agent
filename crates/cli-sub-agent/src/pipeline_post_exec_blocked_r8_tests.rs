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
