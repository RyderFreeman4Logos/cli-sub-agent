use csa_session::{MetaSessionState, SessionResult};

use super::PostExecContext;

/// True for a SA-mode `run` that mutated files but produced no positive
/// structured completion receipt. Such a run cannot be trusted as completed.
pub(super) fn dirty_sa_run_lacks_completion_receipt(
    sa_mode: bool,
    task_type: Option<&str>,
    changed_paths: &[String],
    has_positive_structured_completion: bool,
) -> bool {
    sa_mode
        && matches!(task_type, None | Some("run"))
        && !changed_paths.is_empty()
        && !has_positive_structured_completion
}

/// Dirty-SA exit-preservation (#2806 R9b-F2): a SA-mode `run` that mutated
/// files must NOT have a nonzero exit incidentally downgraded to success,
/// regardless of whether a structured completion receipt is present. A valid
/// receipt suppresses the exit-0 dirty-run-unconfirmed rewrite (handled by
/// [`dirty_sa_run_lacks_completion_receipt`]), but it does not license
/// downgrading a real nonzero failure on a dirty SA run.
///
/// This MUST run BEFORE the effective-outcome classification so it sees the raw
/// nonzero exit. It records a CSA-own gate failure, which the classifier treats
/// as authoritative-fatal (never downgraded).
pub(super) fn maybe_mark_dirty_sa_run_without_receipt(
    ctx: &PostExecContext<'_>,
    session: &mut MetaSessionState,
    result: &mut csa_process::ExecutionResult,
    session_result: &mut SessionResult,
    has_positive_structured_completion: bool,
) {
    // Dirty-SA exit-preservation: a nonzero exit on a dirty SA-mode run is
    // authoritative. A valid receipt does not downgrade it (#2806 R9b-F2).
    let dirty_sa_run =
        ctx.sa_mode && matches!(ctx.task_type, None | Some("run")) && !ctx.changed_paths.is_empty();
    if dirty_sa_run && result.exit_code != 0 {
        // Capture the raw exit BEFORE marking the gate: `mark_gate_failure`
        // forces exit_code to 1, but a timeout (124) / signal (137/143) exit
        // must be preserved verbatim for status mapping and diagnostics
        // (#2806 R9b-F2). The CSA gate marker makes the outcome classifier
        // treat this as authoritative-fatal (never downgraded) without
        // clobbering the specific terminal code. `session_result` was already
        // initialized with the terminal-reason-derived status
        // (initial_session_status, e.g. "timed_out"/"interrupted"), so preserve
        // that status instead of recomputing from status_from_exit_code (which
        // would map 124 -> "failure", losing the timeout/signal distinction).
        //
        // R10-F3: result-contract enforcement runs BEFORE this function and may
        // have called `mark_gate_failure`, coercing `exit_code` to 1. Use the
        // ORIGINAL exit code captured pre-contract (`ctx.original_exit_code`)
        // when available so a timeout/signal exit is preserved verbatim, not
        // flattened to the contract's generic 1 (#2806 R10-F3).
        let preserved_exit_code = ctx.original_exit_code.unwrap_or(result.exit_code);
        tracing::warn!(
            session = %session.meta_session_id,
            exit_code = preserved_exit_code,
            has_receipt = has_positive_structured_completion,
            "Dirty SA-mode run exited nonzero — preserving nonzero exit (rejecting incidental downgrade)"
        );
        result.mark_gate_failure("dirty-sa-nonzero-exit-preserved");
        result.exit_code = preserved_exit_code;
        session_result.exit_code = preserved_exit_code;
        if let Some(tool_state) = session.tools.get_mut(ctx.executor.tool_name()) {
            tool_state.last_exit_code = preserved_exit_code;
        }
        return;
    }

    // Exit-0 dirty-run-unconfirmed: a SA-mode run that mutated files but lacks
    // a positive structured completion receipt cannot be trusted as completed.
    if result.exit_code != 0
        || !dirty_sa_run_lacks_completion_receipt(
            ctx.sa_mode,
            ctx.task_type,
            &ctx.changed_paths,
            has_positive_structured_completion,
        )
    {
        return;
    }

    let unconfirmed_summary = format!(
        "dirty SA-mode run lacks a positive structured completion signal; task was not \
         completed. Original summary: {}",
        result.summary,
    );
    tracing::warn!(
        session = %session.meta_session_id,
        original_summary = %result.summary,
        "Dirty SA-mode run lacks a structured completion receipt — rewriting exit_code to 1"
    );
    session_result.exit_code = 1;
    session_result.status = csa_session::SessionResult::status_from_exit_code(1);
    session_result.summary = unconfirmed_summary.clone();
    result.mark_gate_failure("dirty-sa-run-unconfirmed");
    result.summary = unconfirmed_summary.clone();
    if let Some(tool_state) = session.tools.get_mut(ctx.executor.tool_name()) {
        tool_state.last_exit_code = 1;
        tool_state.last_action_summary = unconfirmed_summary;
    }
}
