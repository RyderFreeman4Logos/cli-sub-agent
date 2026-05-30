//! Unit tests for the effective session-outcome classifier (#161).

use super::*;

/// A CSA-own gate failure is always authoritative-fatal, regardless of the
/// model having completed or the exit code value. This is the core regression
/// guard: gate failures MUST NOT be downgraded.
#[test]
fn gate_failure_is_always_authoritative() {
    for task_kind in [SessionTaskKind::Run, SessionTaskKind::ReviewOrDebate] {
        let outcome =
            classify_effective_session_outcome(Some(true), 1, Some("edit-guard"), task_kind, true);
        assert_eq!(outcome, EffectiveOutcome::ExitCodeAuthoritative);
    }
}

/// A gate failure stays fatal even if the raw exit code is somehow 0 — the
/// marker, not the exit code, decides.
#[test]
fn gate_failure_authoritative_even_with_zero_exit() {
    let outcome = classify_effective_session_outcome(
        Some(true),
        0,
        Some("no-op"),
        SessionTaskKind::Run,
        true,
    );
    assert_eq!(outcome, EffectiveOutcome::ExitCodeAuthoritative);
}

#[test]
fn timeout_and_signal_exits_are_authoritative() {
    for code in [124, 137, 143] {
        let outcome =
            classify_effective_session_outcome(Some(true), code, None, SessionTaskKind::Run, true);
        assert_eq!(
            outcome,
            EffectiveOutcome::ExitCodeAuthoritative,
            "exit {code} must stay authoritative"
        );
    }
}

/// The #1661 scenario: model completed, an incidental hook/in-turn command
/// exited nonzero, no gate fired. A bare run downgrades to success.
#[test]
fn incidental_nonzero_on_completed_run_downgrades() {
    let outcome =
        classify_effective_session_outcome(Some(true), 1, None, SessionTaskKind::Run, true);
    assert_eq!(
        outcome,
        EffectiveOutcome::IncidentalDowngrade { raw_exit_code: 1 }
    );
}

/// A run downgrades even without captured final output: a completed turn is the
/// deliverable for a run.
#[test]
fn incidental_run_downgrades_without_output() {
    let outcome =
        classify_effective_session_outcome(Some(true), 1, None, SessionTaskKind::Run, false);
    assert_eq!(
        outcome,
        EffectiveOutcome::IncidentalDowngrade { raw_exit_code: 1 }
    );
}

/// Review/debate only downgrade when the deliverable (verdict/answer) is
/// present; a completed turn alone is insufficient.
#[test]
fn review_downgrades_only_with_deliverable() {
    let with_deliverable = classify_effective_session_outcome(
        Some(true),
        1,
        None,
        SessionTaskKind::ReviewOrDebate,
        true,
    );
    assert_eq!(
        with_deliverable,
        EffectiveOutcome::IncidentalDowngrade { raw_exit_code: 1 }
    );

    let without_deliverable = classify_effective_session_outcome(
        Some(true),
        1,
        None,
        SessionTaskKind::ReviewOrDebate,
        false,
    );
    assert_eq!(without_deliverable, EffectiveOutcome::ExitCodeAuthoritative);
}

/// Model explicitly did not complete and produced no output → fail fast even
/// when the exit code is 0.
#[test]
fn incomplete_model_without_output_force_fails() {
    let outcome =
        classify_effective_session_outcome(Some(false), 0, None, SessionTaskKind::Run, false);
    assert_eq!(outcome, EffectiveOutcome::ForceFailure);
}

/// Model did not complete but produced final output → not a force-failure; the
/// exit code stands (0 → success, nonzero → authoritative).
#[test]
fn incomplete_model_with_output_is_authoritative() {
    let zero = classify_effective_session_outcome(Some(false), 0, None, SessionTaskKind::Run, true);
    assert_eq!(zero, EffectiveOutcome::ExitCodeAuthoritative);
}

/// Undetermined completion (legacy gemini-cli, no envelope → `None`) never
/// force-fails; the exit code is authoritative.
#[test]
fn undetermined_completion_defers_to_exit_code() {
    let zero = classify_effective_session_outcome(None, 0, None, SessionTaskKind::Run, true);
    assert_eq!(zero, EffectiveOutcome::ExitCodeAuthoritative);

    // Nonzero + undetermined completion does NOT downgrade — only an explicit
    // `model_completed == true` earns a downgrade.
    let nonzero = classify_effective_session_outcome(None, 1, None, SessionTaskKind::Run, true);
    assert_eq!(nonzero, EffectiveOutcome::ExitCodeAuthoritative);
}

#[test]
fn clean_exit_is_authoritative() {
    let outcome =
        classify_effective_session_outcome(Some(true), 0, None, SessionTaskKind::Run, true);
    assert_eq!(outcome, EffectiveOutcome::ExitCodeAuthoritative);
}

#[test]
fn task_kind_mapping() {
    assert_eq!(task_kind_from_task_type(None), SessionTaskKind::Run);
    assert_eq!(task_kind_from_task_type(Some("run")), SessionTaskKind::Run);
    assert_eq!(
        task_kind_from_task_type(Some("review")),
        SessionTaskKind::ReviewOrDebate
    );
    assert_eq!(
        task_kind_from_task_type(Some("debate")),
        SessionTaskKind::ReviewOrDebate
    );
}

#[test]
fn downgrade_note_preserves_raw_exit_and_reason() {
    let note = incidental_downgrade_note(1, Some("end_turn"));
    assert!(note.contains("(1)"), "raw exit code present: {note}");
    assert!(note.contains("end_turn"), "terminal reason present: {note}");

    let no_reason = incidental_downgrade_note(2, None);
    assert!(no_reason.contains("(2)"));
    assert!(no_reason.contains("model completed"));
}
