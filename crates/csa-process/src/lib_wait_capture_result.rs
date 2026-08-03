use std::process::ExitStatus;

use super::signal_exit::{append_signal_exit_note, process_exit_status};
use super::{
    ExecutionResult, SpoolRotator, append_actionable_detail_for_opaque_payload, extract_summary,
    failure_summary, parse_legacy_terminal_reason, resolve_actionable_failure_detail,
    sanitize_opaque_object_payloads, sanitize_spool_plan,
};

pub(super) struct CapturedExecution {
    pub(super) output: String,
    pub(super) stderr_output: String,
    pub(super) persistent_rate_limit_note: Option<String>,
    pub(super) idle_timed_out: bool,
    pub(super) timeout_note: String,
    pub(super) child_exited_early: bool,
    pub(super) child_exited_early_note: String,
    pub(super) workspace_boundary_timed_out: bool,
    pub(super) workspace_boundary_note: String,
    pub(super) status: ExitStatus,
    pub(super) spool_file: Option<SpoolRotator>,
    pub(super) stderr_spool_file: Option<SpoolRotator>,
}

pub(super) fn finalize_captured_execution(input: CapturedExecution) -> ExecutionResult {
    let CapturedExecution {
        output,
        mut stderr_output,
        persistent_rate_limit_note,
        idle_timed_out,
        timeout_note,
        child_exited_early,
        child_exited_early_note,
        workspace_boundary_timed_out,
        workspace_boundary_note,
        status,
        mut spool_file,
        mut stderr_spool_file,
    } = input;
    let process_exit = process_exit_status(status);
    let mut exit_code = process_exit.code;
    if let Some(note) = persistent_rate_limit_note.as_deref() {
        exit_code = 1;
        if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
            stderr_output.push('\n');
        }
        stderr_output.push_str(note);
        stderr_output.push('\n');
    } else if idle_timed_out {
        exit_code = 137;
        if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
            stderr_output.push('\n');
        }
        stderr_output.push_str(&timeout_note);
        stderr_output.push('\n');
    } else if child_exited_early {
        if exit_code == 0 {
            exit_code = 1;
        }
        if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
            stderr_output.push('\n');
        }
        stderr_output.push_str(&child_exited_early_note);
        stderr_output.push('\n');
    } else if workspace_boundary_timed_out {
        if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
            stderr_output.push('\n');
        }
        stderr_output.push_str(&workspace_boundary_note);
        stderr_output.push('\n');
    } else if let Some(note) = process_exit.note.as_deref() {
        append_signal_exit_note(&mut stderr_output, note);
    }

    let summary = if let Some(note) = persistent_rate_limit_note {
        note
    } else if idle_timed_out {
        timeout_note
    } else if child_exited_early {
        child_exited_early_note.clone()
    } else if let Some(note) = process_exit.note.clone() {
        note
    } else if exit_code == 0 {
        extract_summary(&output)
    } else if workspace_boundary_timed_out {
        workspace_boundary_note
    } else {
        failure_summary(&output, &stderr_output, exit_code)
    };

    let raw_process_exit_code = exit_code;
    let terminal_reason = if idle_timed_out || workspace_boundary_timed_out {
        Some("idle_timeout".to_string())
    } else if process_exit.signal.is_some() {
        Some("signal".to_string())
    } else {
        parse_legacy_terminal_reason(&output)
    };
    let model_completed =
        if idle_timed_out || workspace_boundary_timed_out || process_exit.signal.is_some() {
            Some(false)
        } else if terminal_reason.is_some() {
            crate::model_completed_from_terminal_reason(terminal_reason.as_deref())
        } else if child_exited_early {
            Some(false)
        } else {
            None
        };

    let output = sanitize_opaque_object_payloads(&output);
    let mut stderr_output = sanitize_opaque_object_payloads(&stderr_output);
    let actionable_detail = resolve_actionable_failure_detail(&summary, exit_code);
    stderr_output = append_actionable_detail_for_opaque_payload(&stderr_output, &actionable_detail);

    let output_spool_plan = spool_file.take().map(SpoolRotator::finalize);
    let stderr_spool_plan = stderr_spool_file.take().map(SpoolRotator::finalize);
    if let Some(plan_result) = output_spool_plan {
        match plan_result {
            Ok(plan) => {
                if let Err(error) = sanitize_spool_plan(plan, None) {
                    tracing::warn!(error = %error, "Failed to sanitize output spool tail");
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "Failed to finalize output spool file");
            }
        }
    }
    if let Some(plan_result) = stderr_spool_plan {
        match plan_result {
            Ok(plan) => {
                if let Err(error) = sanitize_spool_plan(plan, Some(&actionable_detail)) {
                    tracing::warn!(error = %error, "Failed to sanitize stderr spool tail");
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "Failed to finalize stderr spool file");
            }
        }
    }

    ExecutionResult {
        output,
        stderr_output,
        summary,
        exit_code,
        raw_process_exit_code: Some(raw_process_exit_code),
        model_completed,
        terminal_reason,
        exit_signal: process_exit.signal,
        peak_memory_mb: None,
        ..Default::default()
    }
}
