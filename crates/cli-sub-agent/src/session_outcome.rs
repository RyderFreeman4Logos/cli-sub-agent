//! Effective session-outcome classifier (#161).
//!
//! A CSA meta-session's exit code is CSA's own contract with its caller: it must
//! reflect whether the session achieved its purpose — the model turn completing
//! plus CSA's own deterministic gates — NOT the raw tool-process exit code. An
//! incidental nonzero exit from a hook (e.g. a SessionStart hook hitting EROFS)
//! or from a command the model ran during its turn must not flip a completed
//! session to `failure`.
//!
//! This module turns the transport-boundary signals on [`ExecutionResult`]
//! (`model_completed`, `exit_code`, `csa_gate_failure`, `terminal_reason`) plus
//! the task kind and final-output presence into an [`EffectiveOutcome`]. The
//! caller applies the outcome to the [`ExecutionResult`] / `SessionResult`.
//!
//! The fatal-gate vs incidental distinction is ALWAYS explicit via
//! `csa_gate_failure`, never inferred from the exit-code value.

/// Whether the session ran a `csa run` task or a `review`/`debate` task. Review
/// and debate only downgrade an incidental exit when their deliverable (a
/// verdict / answer) is present; a bare `run` needs only a completed turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionTaskKind {
    Run,
    ReviewOrDebate,
}

/// The classifier's verdict on how `exit_code` should be interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffectiveOutcome {
    /// `exit_code` is authoritative; derive status from it as-is. Covers clean
    /// exits, CSA-own gate failures, and timeout/signal kills.
    ExitCodeAuthoritative,
    /// A completed turn exited nonzero for an incidental reason (failing hook or
    /// in-turn command). Downgrade to success, record a warning, and preserve
    /// `raw_exit_code` as a diagnostic.
    IncidentalDowngrade { raw_exit_code: i32 },
    /// The transport reported the model did not complete and produced no final
    /// output. Fail fast even if the raw exit code is `0`.
    ForceFailure,
}

/// Exit codes that denote a wall-clock timeout (124) or a SIGKILL/SIGTERM kill
/// (137/143). These are always real failures and never downgraded.
fn is_timeout_or_signal(exit_code: i32) -> bool {
    matches!(exit_code, 124 | 137 | 143)
}

/// Classify the effective outcome of a session from its transport-boundary
/// signals. Pure: the caller applies the result.
///
/// Decision order (first match wins):
/// 1. `csa_gate_failure` set → a CSA-own gate fired; `exit_code` is
///    authoritative-fatal and is never reinterpreted.
/// 2. timeout / signal exit (124/137/143) → authoritative failure.
/// 3. model explicitly did not complete AND no final output → fail fast.
/// 4. `exit_code == 0` → authoritative success.
/// 5. nonzero exit, not a gate, not timeout/signal, model completed, and the
///    task's deliverable is present → incidental → downgrade.
/// 6. otherwise → authoritative (the nonzero exit stands).
pub(crate) fn classify_effective_session_outcome(
    model_completed: Option<bool>,
    exit_code: i32,
    csa_gate_failure: Option<&str>,
    task_kind: SessionTaskKind,
    final_output_present: bool,
) -> EffectiveOutcome {
    if csa_gate_failure.is_some() || is_timeout_or_signal(exit_code) {
        return EffectiveOutcome::ExitCodeAuthoritative;
    }
    if model_completed == Some(false) && !final_output_present {
        return EffectiveOutcome::ForceFailure;
    }
    if exit_code == 0 {
        return EffectiveOutcome::ExitCodeAuthoritative;
    }
    let deliverable_present = match task_kind {
        SessionTaskKind::Run => true,
        SessionTaskKind::ReviewOrDebate => final_output_present,
    };
    if model_completed == Some(true) && deliverable_present {
        return EffectiveOutcome::IncidentalDowngrade {
            raw_exit_code: exit_code,
        };
    }
    EffectiveOutcome::ExitCodeAuthoritative
}

/// Map a `csa run`-style `task_type` to a [`SessionTaskKind`]. `None` and
/// `"run"` are runs; everything else (`"review"`, `"debate"`) is a deliverable
/// task.
pub(crate) fn task_kind_from_task_type(task_type: Option<&str>) -> SessionTaskKind {
    match task_type {
        None | Some("run") => SessionTaskKind::Run,
        _ => SessionTaskKind::ReviewOrDebate,
    }
}

/// Human-readable warning recorded when an incidental nonzero exit is downgraded
/// to success. Preserves the raw exit code and the terminal reason for audit.
pub(crate) fn incidental_downgrade_note(
    raw_exit_code: i32,
    terminal_reason: Option<&str>,
) -> String {
    let reason = terminal_reason.unwrap_or("model completed");
    format!(
        "incidental nonzero exit ({raw_exit_code}) on a completed turn ({reason}); \
         treated as success — the model turn reached a terminal state and no CSA gate failed"
    )
}

#[cfg(test)]
#[path = "session_outcome_tests.rs"]
mod tests;
