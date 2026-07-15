use std::path::Path;

use anyhow::{Context, Result};
use csa_executor::TransportResult;
use csa_session::{MetaSessionState, SessionResult};

use crate::pipeline::SessionExecutionResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub(super) enum CompletionPlan {
    Legacy,
    CleanRoom,
}

#[cfg(test)]
impl CompletionPlan {
    pub(super) const fn effect_names(self) -> &'static [&'static str] {
        match self {
            Self::Legacy => &[
                "post-run-hooks",
                "commit-guards",
                "history-handoff",
                "memory-persistence",
                "cooldown",
            ],
            Self::CleanRoom => &["minimal-result"],
        }
    }
}

pub(super) struct CleanRoomCompletionPlan {
    pub(super) execution_start_time: chrono::DateTime<chrono::Utc>,
    pub(super) timeout_diagnostics: crate::session_kill_diagnostics::TimeoutDiagnostics,
}

pub(super) fn complete_clean_room_session(
    project_root: &Path,
    session: &mut MetaSessionState,
    tool_name: &str,
    transport: TransportResult,
    plan: CleanRoomCompletionPlan,
) -> Result<SessionExecutionResult> {
    let completed_at = chrono::Utc::now();
    let mut execution = transport.execution;
    let failure_reason = classify_failure(tool_name, &execution);
    if failure_reason.is_some() && execution.exit_code == 0 {
        execution.exit_code = 1;
    }
    let status = if failure_reason.is_some() || execution.exit_code != 0 {
        "failure"
    } else {
        "success"
    };
    let summary = failure_reason
        .map(str::to_string)
        .or_else(|| (!execution.summary.is_empty()).then(|| execution.summary.clone()))
        .unwrap_or_else(|| format!("clean-room execution {status}"));
    let mut persisted = SessionResult {
        post_exec_gate: None,
        status: status.to_string(),
        exit_code: execution.exit_code,
        summary,
        tool: tool_name.to_string(),
        started_at: plan.execution_start_time,
        completed_at,
        events_count: transport.events.len() as u64,
        artifacts: Vec::new(),
        peak_memory_mb: execution.peak_memory_mb,
        ..Default::default()
    };
    crate::session_kill_diagnostics::save_result_with_signal_diagnostic(
        project_root,
        session,
        tool_name,
        &mut persisted,
        execution.terminal_reason.as_deref(),
        Some(&plan.timeout_diagnostics),
        Some(&mut execution.stderr_output),
    )
    .context("persist minimal clean-room result")?;

    session.turn_count = session.turn_count.saturating_add(1);
    session.last_accessed = completed_at;
    session.termination_reason = execution
        .terminal_reason
        .clone()
        .or_else(|| failure_reason.map(str::to_string));
    csa_session::save_session(session).context("persist terminal clean-room session state")?;

    Ok(SessionExecutionResult {
        execution,
        meta_session_id: session.meta_session_id.clone(),
        provider_session_id: transport.provider_session_id,
        changed_paths: Some(Vec::new()),
        commit_created: Some(false),
    })
}

pub(super) fn complete_clean_room_error(
    project_root: &Path,
    session: &mut MetaSessionState,
    tool_name: &str,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    error: &anyhow::Error,
) -> Result<()> {
    let completed_at = chrono::Utc::now();
    let mut result = SessionResult {
        post_exec_gate: None,
        status: "failure".to_string(),
        exit_code: 1,
        summary: format!("clean-room transport: {error}"),
        tool: tool_name.to_string(),
        started_at: execution_start_time,
        completed_at,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };
    crate::session_kill_diagnostics::save_result_with_signal_diagnostic(
        project_root,
        session,
        tool_name,
        &mut result,
        None,
        None,
        None,
    )
    .context("persist minimal clean-room transport failure")?;
    session.last_accessed = completed_at;
    session.termination_reason = Some("transport_error".to_string());
    csa_session::save_session(session).context("persist failed clean-room session state")
}

fn classify_failure(
    tool_name: &str,
    execution: &csa_process::ExecutionResult,
) -> Option<&'static str> {
    if execution.output.trim().is_empty() {
        return Some("clean-room execution produced no review artifact");
    }
    if execution.model_completed == Some(false) {
        return Some("clean-room model turn did not complete");
    }
    let exhaustion_text = format!("{}\n{}", execution.output, execution.stderr_output);
    if crate::run_cmd_post::detect_permanent_tool_exhaustion_text(
        tool_name,
        &exhaustion_text,
        execution.exit_code,
        execution.terminal_reason.as_deref(),
    )
    .is_some()
    {
        return Some("clean-room tool is permanently exhausted");
    }
    None
}
