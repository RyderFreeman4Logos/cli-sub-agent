//! False-success detection gates applied after the effective-outcome
//! classification.
//!
//! These three gates re-examine a session whose exit survived the #161
//! incidental-downgrade classifier (exit still 0, no CSA gate fired by the
//! classifier) and promote genuine false-successes to real failures:
//!
//! - **no-op gate**: short SA-mode `run` with zero useful work.
//! - **blocked-output gate**: exit-0 worker that printed `STATUS: BLOCKED`.
//! - **no-progress gate**: exit-0 `run` with no git diff/commits since start.
//!
//! Extracted from `pipeline_post_exec.rs` to keep that module under the
//! monolith token budget (#2806 R9b).

use csa_session::{MetaSessionState, SessionResult};

use super::PostExecContext;

/// Apply the post-classification false-success gates in order: no-op,
/// blocked-output, then no-progress. Each gate independently calls
/// `result.mark_gate_failure` when it promotes a real failure, which also
/// clears any pending incidental-downgrade warning on `result`.
pub(super) fn apply_false_success_gates(
    ctx: &PostExecContext<'_>,
    session: &mut MetaSessionState,
    result: &mut csa_process::ExecutionResult,
    session_result: &mut SessionResult,
    elapsed_secs: i64,
    has_positive_structured_completion: bool,
    has_meaningful_reasoning_output: bool,
) {
    // No-op gate: fail short successful SA-mode runs with no tool calls/output.
    if ctx.sa_mode
        && ctx.task_type.is_none_or(|t| t == "run")
        && result.exit_code == 0
        && !has_positive_structured_completion
        && session.turn_count <= 1
        && !ctx.has_tool_calls
        && !has_meaningful_reasoning_output
        && ctx.changed_paths.is_empty()
        && elapsed_secs < super::no_op::ELAPSED_THRESHOLD_SECS
    {
        let original_summary = session_result.summary.clone();
        let no_op_summary = super::no_op::build_no_op_failure_summary(
            session.turn_count,
            elapsed_secs,
            ctx.executor.tool_name(),
            session.description.as_deref(),
            ctx.prompt,
            &original_summary,
        );
        tracing::warn!(
            session = %session.meta_session_id,
            turn_count = session.turn_count,
            elapsed_secs,
            "SA-mode no-op exit gate triggered — rewriting status to failure"
        );
        session_result.exit_code = 1;
        session_result.status = SessionResult::status_from_exit_code(1);
        session_result.summary = no_op_summary.clone();
        result.summary = no_op_summary.clone();
        // CSA-own gate: a SA-mode no-op (zero useful work) is a real failure.
        result.mark_gate_failure("no-op-exit");
        // Sync tool_state so state.toml agrees with result.toml after rewrite.
        if let Some(tool_state) = session.tools.get_mut(ctx.executor.tool_name()) {
            tool_state.last_exit_code = 1;
            tool_state.last_action_summary = no_op_summary;
        }
    }
    // Fail zero-exit workers that report a blocker on any output stream.
    if result.exit_code == 0
        && super::blocked::worker_output_indicates_blocked_with_receipt(
            &result.output,
            &result.stderr_output,
            &result.summary,
            has_positive_structured_completion,
        )
    {
        let blocked_summary = format!(
            "worker blocked: STATUS: BLOCKED detected; task was not completed. \
             Original summary: {}",
            result.summary,
        );
        tracing::warn!(
            session = %session.meta_session_id,
            original_summary = %result.summary,
            "STATUS: BLOCKED in session output — rewriting exit_code to 1"
        );
        session_result.exit_code = 1;
        session_result.status = csa_session::SessionResult::status_from_exit_code(1);
        session_result.summary = blocked_summary.clone();
        // CSA-own gate: worker reported STATUS: BLOCKED — a real failure.
        result.mark_gate_failure("worker-blocked");
        result.summary = blocked_summary.clone();
        if let Some(tool_state) = session.tools.get_mut(ctx.executor.tool_name()) {
            tool_state.last_exit_code = 1;
            tool_state.last_action_summary = blocked_summary;
        }
    }
    if result.exit_code == 0
        && ctx.task_type == Some("run")
        && !has_positive_structured_completion
        && session.turn_count <= 1
        && !ctx.has_tool_calls
        && !has_meaningful_reasoning_output
        && ctx.changed_paths.is_empty()
        && elapsed_secs < super::no_op::ELAPSED_THRESHOLD_SECS
        && let Err(err) = super::progress::maybe_mark_no_progress_session(
            ctx.project_root,
            session,
            result,
            &mut *session_result,
        )
    {
        tracing::warn!(
            session = %session.meta_session_id,
            error = %err,
            "Skipping post-session no-progress detection; preserving success status"
        );
    }
}
