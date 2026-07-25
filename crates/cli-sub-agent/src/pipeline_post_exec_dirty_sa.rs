use csa_session::{MetaSessionState, SessionResult};

use super::PostExecContext;

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

pub(super) fn maybe_mark_dirty_sa_run_without_receipt(
    ctx: &PostExecContext<'_>,
    session: &mut MetaSessionState,
    result: &mut csa_process::ExecutionResult,
    session_result: &mut SessionResult,
    has_positive_structured_completion: bool,
) {
    if !dirty_sa_run_lacks_completion_receipt(
        ctx.sa_mode,
        ctx.task_type,
        &ctx.changed_paths,
        has_positive_structured_completion,
    ) {
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
