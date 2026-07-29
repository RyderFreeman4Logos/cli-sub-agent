//! R15 regressions for per-occurrence historical qualifier binding and
//! position-aware `now`/`currently` override (#2806).
//!
//! Two HIGH review findings:
//!
//! - **R15-F1 (per-occurrence binding):** A whole-clause historical-qualifier
//!   scan over-suppresses a CONCURRENT veto in a mixed clause without
//!   punctuation. The historical qualifier is now bound to the occurrence it is
//!   syntactically associated with (same coordination segment), so a current
//!   veto after a coordinating conjunction is flagged.
//! - **R15-F2 (position-aware override):** A whole-clause `now`/`currently` scan
//!   reclassifies a historical status as concurrent. Commas are NOT clause
//!   boundaries, so `Previous gate status unknown, now gate passed.` is ONE
//!   clause. The override is now LOCAL to the matched `status [is] <modifier>
//!   <state>` claim — a `now`/`currently` belonging to a later pass does NOT
//!   override the historical status.

use super::{message_reports_gate_resolution, worker_output_indicates_blocked_with_receipt};

// ===========================================================================
// R15-F1: per-occurrence historical qualifier binding (mixed same-clause)
// ===========================================================================

#[test]
fn r15_mixed_same_clause_historical_failure_and_current_unresolved_signal_blocks() {
    // R15-F1: `previous` modifies `attempt failed`; the coordinating `and`
    // introduces a NEW gate subject with the concurrent `remains blocked`. The
    // whole-clause qualifier scan used to treat the entire clause as historical,
    // hiding the concurrent veto. Per-occurrence binding flags it.
    for message in [
        "Previous attempt failed and gate remains blocked; gate passed.",
        "Previous attempt failed but gate remains blocked; gate passed.",
        "Earlier run failed and gate still blocked; gate passed.",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "mixed same-clause: concurrent `remains/still blocked` must veto the pass: {message}"
        );
        let agent_message = format!(r#"{{"agent_message":"{message}"}}"#);
        assert!(
            worker_output_indicates_blocked_with_receipt(&agent_message, "", "retry ok", true),
            "current receipt + concurrent unresolved signal must still block: {message}"
        );
    }
}

#[test]
fn r15_mixed_same_clause_historical_failure_and_current_omission_blocks() {
    // R15-F1: `previous` modifies `failure was fixed`; the adversative `but`
    // introduces a NEW current omission (`tests and commit omitted`). The
    // whole-clause qualifier scan used to exempt it. Per-occurrence binding
    // flags the current omission.
    for message in [
        "Previous failure was fixed but tests and commit omitted; gate passed.",
        "Earlier failure was fixed but tests and commit omitted; gate passed.",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "mixed same-clause: concurrent omission must veto the pass: {message}"
        );
    }
}

#[test]
fn r15_mixed_same_clause_historical_unresolved_and_current_failure_blocks() {
    // R15-F1: `previous` governs `could not confirm`; the `and` boundary
    // introduces a NEW gate subject with the concurrent `failed`. The current
    // failure must be flagged.
    let message = "Previous attempt could not confirm and gate failed; gate passed.";
    assert!(
        !message_reports_gate_resolution(message),
        "mixed same-clause: concurrent failure after `and gate` must veto: {message}"
    );
}

#[test]
fn r15_mixed_same_clause_historical_omission_and_current_unresolved_blocks() {
    // R15-F1: `previous` governs the omission segment; the `but` boundary
    // introduces a concurrent `remains blocked`. The concurrent unresolved
    // signal must be flagged.
    let message = "Previous turn omitted tests and commit but gate remains blocked; gate passed.";
    assert!(
        !message_reports_gate_resolution(message),
        "mixed same-clause: concurrent `remains blocked` after `but` must veto: {message}"
    );
}

#[test]
fn r15_mixed_same_clause_historical_unknown_and_current_unknown_blocks() {
    // R15-F1: `previous` governs the first `status unknown`; the `but` boundary
    // introduces a concurrent `gate status unknown`. The second occurrence must
    // be flagged as concurrent.
    let message = "Previous status unknown but gate status unknown; gate passed.";
    assert!(
        !message_reports_gate_resolution(message),
        "mixed same-clause: concurrent `gate status unknown` after `but` must veto: {message}"
    );
}

// ===========================================================================
// R15-F1 non-regression: single-occurrence historical clauses still resolve
// ===========================================================================

#[test]
fn r15_single_historical_occurrence_clause_still_resolves() {
    // Non-regression: a clause with a SINGLE veto occurrence governed by a
    // historical qualifier (no coordinating conjunction boundary to a new
    // subject) must still resolve. Per-occurrence binding must not over-reject.
    // These messages each carry a terminal gate pass AND only historical veto
    // narration, so they must resolve.
    for message in [
        // Historical failure narration (R13/R14 contract).
        "The failure occurred in the previous attempt. Gate passed.",
        "Previous turn failed. Gate passed.",
        "The gate previously failed. Gate passed.",
        // Historical unresolved narration (R14 contract).
        "Prior attempt remains blocked. Gate passed.",
        "Previous attempt could not confirm gate pass. Gate passed.",
        "This earlier run could not confirm and failed. Gate passed.",
        "Earlier it did not pass. Gate passed.",
        // Historical omission narration that does not veto a pass (R10/R14
        // contract) — the omission is governed by the historical qualifier and
        // the terminal pass stands.
        "gate passed. previous turn omitted tests and commit; this turn completed both",
        // Historical unknown narration (R13/R14 contract).
        "Previous gate status unknown. Gate passed.",
        "Prior status unavailable. Gate passed.",
    ] {
        assert!(
            message_reports_gate_resolution(message),
            "single historical occurrence with a pass signal must resolve: {message}"
        );
    }
}

// ===========================================================================
// R15-F2: position-aware now/currently override
// ===========================================================================

#[test]
fn r15_position_unaware_now_does_not_reclassify_historical_status() {
    // R15-F2: a comma is NOT a clause boundary, so the whole thing is ONE
    // clause. `previous` makes `status unknown` historical. `now` belongs to
    // the later `gate passed` claim — it does NOT sit between `status` and the
    // state token, so it must NOT override the historical attribute. The valid
    // pass resolves.
    for message in [
        "Previous gate status unknown, now gate passed.",
        "Previous gate status unknown, gate passed now.",
        "Prior gate status unknown, now gate passed.",
    ] {
        assert!(
            message_reports_gate_resolution(message),
            "a `now` outside the matched status claim must not reclassify a historical status: {message}"
        );
    }
}

#[test]
fn r15_position_unaware_currently_does_not_reclassify_historical_status() {
    // R15-F2: `currently` belonging to the later pass claim (comma-joined, so
    // ONE clause) must not override the historical status in its own segment.
    // `currently` does NOT sit between `status` and the state token, so the
    // historical `status unknown` stays exempted and the valid pass resolves.
    let message = "Previous gate status unknown, currently gate passed.";
    assert!(
        message_reports_gate_resolution(message),
        "`currently` outside the matched status claim must not reclassify a historical status: {message}"
    );
}

#[test]
fn r15_local_current_tense_modifier_still_marks_concurrent_unknown() {
    // Non-regression (R14 contract): a current-tense modifier INSIDE the
    // matched `status [is] <modifier> <state>` claim still marks it concurrent
    // and vetoes the pass.
    for message in [
        "prior status is currently unknown",
        "Gate passed. Gate status is currently unknown.",
        "Previous turn failed. Gate status currently unknown; gate passed.",
        "gate passed, but gate status is currently unknown",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "a LOCAL current-tense modifier inside the status claim must still veto: {message}"
        );
    }
}

#[test]
fn r15_mixed_clause_with_local_modifier_flags_current() {
    // R15-F1 + R15-F2 combined: the second segment's `status is currently
    // unknown` has a LOCAL modifier and is concurrent; it must veto even though
    // a historical qualifier governs the first segment.
    let message = "Previous attempt failed and status is currently unknown; gate passed.";
    assert!(
        !message_reports_gate_resolution(message),
        "local current-tense modifier in a later segment must veto: {message}"
    );
}
