//! R14 convergence unification regressions for clause-scoped veto/resolution.
//!
//! Root cause (#2806 R10→R14 convergence protocol): the four veto/resolution
//! checks used THREE different scopes — whole-message `contains`, ±2-token
//! qualifier windows, and per-line. R14 unifies them onto a single shared
//! per-clause classifier so mixed historical/current messages are classified
//! consistently. These tests pin each of the three HIGH findings plus mixed
//! historical/current messages on the same line.

use super::{message_reports_gate_resolution, worker_output_indicates_blocked_with_receipt};

// ---- Finding 1 HIGH: current-tense modifiers in status-unknown detection ----

/// `gate_signal.rs:reports_concurrent_status_unknown` must recognize optional
/// current-tense modifiers (`currently`/`now`) between the `status` subject and
/// the state token, so "status currently unknown" and "status now unavailable"
/// are detected as concurrent unknowns and veto a pass (#2806 R14-F1).
#[test]
fn r14_current_tense_modifier_status_unknown_vetoes_pass() {
    for message in [
        "Gate passed. Gate status is currently unknown.",
        "Gate passed. Gate status currently unknown.",
        "Gate passed. Gate status now unavailable.",
        "Gate passed. Gate status is now unavailable.",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "a current-tense status-unknown modifier must veto the pass: {message}"
        );
    }
}

/// The receipt path (`worker_output_indicates_blocked_with_receipt`) must also
/// keep a current-tense status unknown blocked, because a concurrent unknown
/// exposes the resolver rather than resolving the gate (#2806 R14-F1).
#[test]
fn r14_current_tense_status_unknown_receipt_path_blocks() {
    for message in [
        "Gate passed. Gate status is currently unknown.",
        "Gate passed. Gate status now unavailable.",
    ] {
        let agent_message = format!(r#"{{"agent_message":"{message}"}}"#);
        assert!(
            worker_output_indicates_blocked_with_receipt(&agent_message, "", "retry ok", true),
            "current-tense status unknown must expose the resolver and block: {message}"
        );
    }
}

/// A historical qualifier must still exempt a pure historical status-unknown
/// claim — the new current-tense modifier handling must not regress the
/// historical exemption (#2806 R8-F2, R13-P1).
#[test]
fn r14_historical_status_unknown_without_current_modifier_still_resolves() {
    for message in [
        "Previous gate status unknown. Gate passed.",
        "Prior status unavailable. Gate passed.",
    ] {
        assert!(
            message_reports_gate_resolution(message),
            "a pure historical status-unknown claim must not veto a later pass: {message}"
        );
    }
}

// ---- Finding 2 HIGH: unified per-clause historical binding for ALL vetoes -----

/// The `unresolved_signal` list (e.g. "could not confirm") is now per-clause.
/// A historical qualifier in an earlier clause must NOT veto a later terminal
/// pass — previously the whole-message `contains` had no scope, so "Previous
/// attempt could not confirm gate pass. Gate passed." stayed blocked (#2806
/// R14-F2).
#[test]
fn r14_historical_unresolved_signal_does_not_veto_later_pass() {
    for message in [
        "Previous attempt could not confirm gate pass. Gate passed.",
        "Prior attempt remains blocked. Gate passed.",
        "Earlier it did not pass. Gate passed.",
    ] {
        assert!(
            message_reports_gate_resolution(message),
            "a historical unresolved signal must not veto a later terminal pass: {message}"
        );
    }
}

/// A CURRENT unresolved signal must still veto a pass — the per-clause move must
/// not weaken detection of a genuine concurrent signal (#2806 R14-F2).
#[test]
fn r14_current_unresolved_signal_still_vetoes_pass() {
    for message in [
        "Gate passed. Gate status remains unknown.",
        "Gate passed. Could not confirm gate pass.",
        "Gate passed. Gate still blocked.",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "a current unresolved signal must still veto the pass: {message}"
        );
    }
}

/// The `failed`/`failure` qualifier binding is now whole-clause, not ±2 tokens.
/// A qualifier further away in the SAME clause must still mark the occurrence
/// as historical, so "The failure occurred in the previous attempt. Gate
/// passed." resolves (#2806 R14-F2).
#[test]
fn r14_historical_failure_distant_qualifier_does_not_veto() {
    for message in [
        "The failure occurred in the previous attempt. Gate passed.",
        "This earlier run could not confirm and failed. Gate passed.",
    ] {
        assert!(
            message_reports_gate_resolution(message),
            "a historical failure with a distant qualifier must not veto a later pass: {message}"
        );
    }
}

// ---- Finding 3 HIGH: per-clause omission check (not per-line) ---------------

/// The omission check is now CLAUSE-scoped (semicolon-delimited), not
/// line-scoped. A historical marker in one clause must NOT exempt a LATER
/// current omission clause on the SAME line (#2806 R14-F3).
#[test]
fn r14_per_clause_omission_historical_marker_does_not_exempt_current() {
    let message =
        "Gate passed. Previous turn omitted tests and commit; tests and commit omitted this turn.";
    assert!(
        !message_reports_gate_resolution(message),
        "a historical omission clause must not exempt a current omission clause on the same line: {message}"
    );
}

/// A pure historical omission (single clause, fixed this turn) must still
/// resolve — the per-clause move must not regress the historical exemption
/// (#2806 R10-F2).
#[test]
fn r14_pure_historical_omission_still_resolves() {
    for message in [
        "The previous turn omitted tests and commit; this turn completed both. Gate passed.",
        "Previously omitted tests and commit. Gate passed.",
    ] {
        assert!(
            message_reports_gate_resolution(message),
            "a pure historical omission must not veto a pass: {message}"
        );
    }
}

/// A CURRENT omission with no historical marker in its clause must still veto a
/// pass — the per-clause move must not weaken detection of a genuine current
/// omission (#2806 R10-F2, R14-F3).
#[test]
fn r14_current_omission_still_vetoes_pass() {
    let message = "Gate passed. Tests and commit omitted.";
    assert!(
        !message_reports_gate_resolution(message),
        "a current omission with no historical marker must veto the pass: {message}"
    );
}

// ---- Mixed historical + current on the same line (convergence scenario) -----

/// Mixed historical narration AND a current concurrent unknown on the same line
/// must keep the message blocked: the current unknown in its own clause exposes
/// the resolver. This is the core convergence scenario — all four veto classes
/// share the SAME per-clause scope, so a historical clause cannot exempt a
/// current clause of any veto class (#2806 R14 convergence).
#[test]
fn r14_mixed_historical_and_current_on_same_line_blocks() {
    // Historical failure narration in clause 1, current status-unknown in clause 2.
    let failure_then_unknown = "Previous turn failed. Gate status currently unknown; gate passed.";
    assert!(
        !message_reports_gate_resolution(failure_then_unknown),
        "a current unknown after historical failure narration must still block: {failure_then_unknown}"
    );

    // Historical omission in clause 1, current omission in clause 2 (Finding 3).
    let omission_mix =
        "Previous turn omitted tests and commit; tests and commit omitted. Gate passed.";
    assert!(
        !message_reports_gate_resolution(omission_mix),
        "a current omission after a historical omission must still block: {omission_mix}"
    );

    // Historical unresolved signal in clause 1, current unresolved signal in clause 2.
    let unresolved_mix =
        "Previous attempt could not confirm. Could not confirm gate pass. Gate passed.";
    assert!(
        !message_reports_gate_resolution(unresolved_mix),
        "a current unresolved signal after a historical one must still block: {unresolved_mix}"
    );
}

/// Conversely, when ALL clauses are historical, the message must resolve — the
/// unified classifier must not over-block purely historical narration (#2806
/// R14 convergence).
#[test]
fn r14_all_historical_clauses_resolve() {
    let message = "Previous turn failed. Previous attempt could not confirm. Gate passed.";
    assert!(
        message_reports_gate_resolution(message),
        "purely historical narration across all clauses must resolve: {message}"
    );
}
