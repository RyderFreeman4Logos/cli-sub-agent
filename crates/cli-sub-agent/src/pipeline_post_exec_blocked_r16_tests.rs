//! R16 regressions: multi-occurrence veto binding (#2806).
//!
//! R16-F1: unresolved-signal matching must inspect every phrase occurrence,
//! not only the first. A historical first match must not hide a concurrent
//! second match after `but`/`and`.
//!
//! R16-F2: omission classification must evaluate each coordination segment
//! independently. A historical first `omitted` must not exempt a concurrent
//! omission after `but`.

use super::{message_reports_gate_resolution, worker_output_indicates_blocked_with_receipt};

#[test]
fn r16_second_unresolved_occurrence_after_historical_first_blocks() {
    for message in [
        "Previous gate remains blocked but gate remains blocked; gate passed.",
        "Previous gate remains blocked but gate still blocked; gate passed.",
        "Earlier attempt could not confirm but could not confirm; gate passed.",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "second concurrent unresolved occurrence must veto: {message}"
        );
        let agent_message = format!(r#"{{"agent_message":"{message}"}}"#);
        assert!(
            worker_output_indicates_blocked_with_receipt(&agent_message, "", "retry ok", true),
            "current receipt must not suppress concurrent second unresolved signal: {message}"
        );
    }
}

#[test]
fn r16_second_omission_after_historical_first_blocks() {
    for message in [
        "Previous turn omitted tests and commit but tests and commit omitted; gate passed.",
        "Earlier turn omitted tests and commit but tests and commit omitted; gate passed.",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "second concurrent omission segment must veto: {message}"
        );
        let agent_message = format!(r#"{{"agent_message":"{message}"}}"#);
        assert!(
            worker_output_indicates_blocked_with_receipt(&agent_message, "", "retry ok", true),
            "current receipt must not suppress concurrent second omission: {message}"
        );
    }
}

#[test]
fn r16_single_historical_unresolved_still_resolves() {
    for message in [
        "Previous gate remains blocked; gate passed.",
        "Earlier attempt could not confirm; gate passed.",
    ] {
        assert!(
            message_reports_gate_resolution(message),
            "pure historical unresolved signal must still resolve: {message}"
        );
    }
}

#[test]
fn r16_single_historical_omission_still_resolves() {
    for message in [
        "Previous turn omitted tests and commit; gate passed.",
        "Earlier turn omitted tests and commit; this turn completed both; gate passed.",
    ] {
        assert!(
            message_reports_gate_resolution(message),
            "pure historical omission must still resolve: {message}"
        );
    }
}
