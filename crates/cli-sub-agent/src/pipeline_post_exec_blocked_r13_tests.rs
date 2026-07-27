//! R13 regressions for clause-scoped blocked-gate resolution.
//!
//! Historical narration in one clause must neither exempt a later current
//! unknown status nor veto a later terminal gate pass.

use super::{message_reports_gate_resolution, worker_output_indicates_blocked_with_receipt};

#[test]
fn r13_cross_clause_historical_qualifier_does_not_exempt_current_unknown() {
    // R13-P1: `prior` belongs to the first clause only. It must not suppress the
    // CURRENT `gate status unknown` claim in the following clause.
    let message = "This was prior. Gate status unknown; gate passed.";
    assert!(
        !message_reports_gate_resolution(message),
        "a historical qualifier from a previous clause must not exempt a current unknown: {message}"
    );
    let agent_message = format!(r#"{{"agent_message":"{message}"}}"#);
    assert!(
        worker_output_indicates_blocked_with_receipt(&agent_message, "", "retry ok", true),
        "current receipt + current unknown must still block: {message}"
    );
}

#[test]
fn r13_historical_failure_does_not_veto_later_gate_pass() {
    // R13-P2: an earlier historical failure is narration, not a current veto of
    // the terminal result in the following clause.
    for message in [
        "Previous turn failed. Gate passed.",
        "Earlier failure. Gate passed.",
        "The gate previously failed. Gate passed.",
    ] {
        assert!(
            message_reports_gate_resolution(message),
            "historical failure must not veto a later terminal gate pass: {message}"
        );
    }

    // Current failures remain vetoes, including both former substring forms.
    for message in [
        "Gate passed. Current gate failed.",
        "Gate passed. Current gate failure.",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "current failure must still veto a gate pass: {message}"
        );
    }

    // Exercise the receipt path too: a historical unknown exposes the resolver,
    // while the prior historical failure must not keep the message blocked.
    let receipt_message = "Previous gate status unknown. Previous turn failed. Gate passed.";
    let agent_message = format!(r#"{{"agent_message":"{receipt_message}"}}"#);
    assert!(
        !worker_output_indicates_blocked_with_receipt(&agent_message, "", "retry ok", true),
        "a later terminal gate pass must resolve historical status/failure narration: {receipt_message}"
    );
}
