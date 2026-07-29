//! R17 regressions: richer `and` coordination-segment boundaries (#2806).
//!
//! R17-1H: `and` must start a new segment not only before bare
//! `gate|status|result`, but also after optional determiners (`the`) and
//! current-turn subjects (`this turn`) and before an `omitted` predicate.
//! Otherwise concurrent vetoes stay glued to a historical qualifier segment.

use super::{message_reports_gate_resolution, worker_output_indicates_blocked_with_receipt};

#[test]
fn r17_and_the_gate_after_historical_failure_blocks() {
    for message in [
        "Previous attempt failed and the gate remains blocked; gate passed.",
        "Earlier attempt failed and the gate still blocked; gate passed.",
        "Previous attempt failed and a gate remains blocked; gate passed.",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "and + determiner + gate subject must start a concurrent segment: {message}"
        );
        let agent_message = format!(r#"{{"agent_message":"{message}"}}"#);
        assert!(
            worker_output_indicates_blocked_with_receipt(&agent_message, "", "retry ok", true),
            "current receipt must not suppress concurrent and-the-gate veto: {message}"
        );
    }
}

#[test]
fn r17_and_this_turn_omitted_after_historical_omission_blocks() {
    for message in [
        "Previous turn omitted tests and commit and this turn omitted tests and commit; gate passed.",
        "Earlier turn omitted tests and commit and this turn omitted tests and commit; gate passed.",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "and + this turn + omitted must start a concurrent omission segment: {message}"
        );
        let agent_message = format!(r#"{{"agent_message":"{message}"}}"#);
        assert!(
            worker_output_indicates_blocked_with_receipt(&agent_message, "", "retry ok", true),
            "current receipt must not suppress concurrent this-turn omission: {message}"
        );
    }
}

#[test]
fn r17_tests_and_commit_still_single_compound_subject() {
    // Regression: compound objects must not split on `and`.
    for message in [
        "Previous turn omitted tests and commit; gate passed.",
        "Earlier failure was fixed but tests and commit omitted; gate passed.",
    ] {
        // First pure historical must resolve; second mixed via `but` must block.
        if message.contains("but") {
            assert!(
                !message_reports_gate_resolution(message),
                "but-separated current omission must still veto: {message}"
            );
        } else {
            assert!(
                message_reports_gate_resolution(message),
                "compound tests-and-commit must not create a false concurrent segment: {message}"
            );
        }
    }
}
