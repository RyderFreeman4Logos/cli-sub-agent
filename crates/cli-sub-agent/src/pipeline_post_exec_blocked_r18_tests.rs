//! R18 regression: an adjectival `omitted` must not create an `and` boundary.
//!
//! An article directly before `omitted` starts a noun phrase (`the omitted
//! tests`), rather than a new omission predicate. The historical qualifier on
//! the first outcome must therefore continue to cover this compound clause.

use super::{message_reports_gate_resolution, worker_output_indicates_blocked_with_receipt};

#[test]
fn r18_and_the_omitted_tests_is_not_a_new_outcome_segment() {
    let message =
        "Previous attempt failed and the omitted tests and commit were completed; gate passed.";

    assert!(
        message_reports_gate_resolution(message),
        "an adjectival omitted noun phrase must not turn historical failure into a current omission: {message}"
    );

    let agent_message = format!(r#"{{"agent_message":"{message}"}}"#);
    assert!(
        !worker_output_indicates_blocked_with_receipt(&agent_message, "", "retry ok", true),
        "a current receipt must preserve the resolved adjectival-omitted message: {message}"
    );
}
