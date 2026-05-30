//! Write-restriction guard violation handling, split out of
//! `pipeline_session_exec` to keep that module under the token budget.
//!
//! When a session runs under an edit / new-file write restriction, these guards
//! revert (tracked-file edits) or remove (newly created files) any out-of-policy
//! changes after the turn, append diagnostics to stderr, and mark the result as
//! a CSA-own gate failure (#161) so the effective-outcome classifier never
//! downgrades the nonzero exit to an incidental success.

use tracing::warn;

use csa_executor::Executor;

use crate::edit_restriction_guard::{NewFileGuard, TrackedFileEditGuard};

/// Apply the edit-restriction (tracked-file) and new-file write guards to a
/// finished turn's `result`. When both guards fire, the edit guard's gate marker
/// wins (first reason recorded) and the new-file guard only overrides the summary
/// if the edit guard left the exit code clean.
pub(super) fn apply_write_restriction_violations(
    edit_guard: Option<TrackedFileEditGuard>,
    new_file_guard: Option<NewFileGuard>,
    executor: &Executor,
    result: &mut csa_process::ExecutionResult,
) -> anyhow::Result<()> {
    if let Some(guard) = edit_guard
        && let Some(violation) = guard.enforce_and_restore()?
    {
        let violation_summary = violation.summary();
        let violation_details = violation.detail_message();
        let previous_summary = result.summary.clone();
        warn!(tool = %executor.tool_name(), "Edit restriction: reverted {n} files", n = violation.modified_paths.len());
        if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        if !previous_summary.trim().is_empty() {
            result.stderr_output.push_str(&format!(
                "Original summary before restriction guard: {previous_summary}\n"
            ));
        }
        result.stderr_output.push_str(&violation_details);
        if !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        result.summary = violation_summary;
        // CSA-own gate: an edit-restriction violation is a real failure; mark it
        // so the #161 effective-outcome classifier never downgrades it.
        result.mark_gate_failure("edit-restriction");
    }
    if let Some(guard) = new_file_guard
        && let Some(violation) = guard.enforce_and_remove()?
    {
        let violation_summary = violation.summary();
        let violation_details = violation.detail_message();
        warn!(
            tool = %executor.tool_name(),
            new_files = violation.new_paths.len(),
            removed = violation.removed_paths.len(),
            "Detected and removed new files created under write restriction"
        );
        if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        result.stderr_output.push_str(&violation_details);
        if !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        // Only override summary if the edit guard didn't already fail.
        if result.exit_code == 0 {
            result.summary = violation_summary;
        }
        // CSA-own gate: a new-file-restriction violation is a real failure. The
        // marker is set only if no earlier gate (e.g. edit guard) set one, so the
        // first violation's reason wins (#161).
        result.mark_gate_failure("new-file-restriction");
    }
    Ok(())
}
