//! Terminal result fallbacks for post-exec failures.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use tracing::warn;

use csa_session::{
    MetaSessionState, PhaseEvent, SessionArtifact, SessionPhase, SessionResult, get_session_dir,
    load_result, load_session, save_result, save_session,
};

const FALLBACK_OUTPUT_TAIL_LINES: usize = 8;
const OUTPUT_LOG_TAIL_READ_BYTES: u64 = 8 * 1024;

pub(crate) fn ensure_terminal_result_for_session_on_post_exec_error(
    project_root: &Path,
    session_id: &str,
    tool_name: &str,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    error: &anyhow::Error,
) {
    let mut session = match load_session(project_root, session_id) {
        Ok(session) => session,
        Err(load_err) => {
            warn!(
                session = %session_id,
                error = %load_err,
                "Failed to load session for post-exec error fallback"
            );
            return;
        }
    };

    ensure_terminal_result_on_post_exec_error(
        project_root,
        &mut session,
        tool_name,
        execution_start_time,
        error,
    );
}

pub(crate) fn build_fallback_result_summary(session_dir: &Path, summary_prefix: &str) -> String {
    match read_output_log_tail(session_dir, FALLBACK_OUTPUT_TAIL_LINES) {
        Some(output_tail) => format!("{summary_prefix}\n\nLast output:\n{output_tail}"),
        None => summary_prefix.to_string(),
    }
}

pub(crate) fn collect_fallback_result_artifacts(
    project_root: &Path,
    session_id: &str,
) -> Vec<SessionArtifact> {
    match csa_session::list_artifacts(project_root, session_id) {
        Ok(artifact_names) => artifact_names
            .into_iter()
            .map(|name| SessionArtifact::new(format!("output/{name}")))
            .collect(),
        Err(err) => {
            warn!(
                session = %session_id,
                error = %err,
                "Failed to enumerate output artifacts for fallback result"
            );
            Vec::new()
        }
    }
}

/// Best-effort fail-safe when post-exec processing returns an error.
///
/// If `result.toml` is missing, persist a synthetic failure result so callers
/// never observe an Active session without a terminal result packet.
pub(crate) fn ensure_terminal_result_on_post_exec_error(
    project_root: &Path,
    session: &mut MetaSessionState,
    tool_name: &str,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    error: &anyhow::Error,
) {
    match load_result(project_root, &session.meta_session_id) {
        Ok(Some(_)) => {
            retire_post_exec_fallback_session(session, chrono::Utc::now(), None);
            return;
        }
        Ok(None) => {}
        Err(load_err) => {
            warn!(
                session = %session.meta_session_id,
                error = %load_err,
                "Failed to read existing result.toml during post-exec error fallback; attempting overwrite"
            );
        }
    }

    let summary_prefix = format!("post-exec: {error}");
    let summary = match get_session_dir(project_root, &session.meta_session_id) {
        Ok(session_dir) => build_fallback_result_summary(&session_dir, &summary_prefix),
        Err(session_dir_err) => {
            warn!(
                session = %session.meta_session_id,
                error = %session_dir_err,
                "Failed to resolve session dir for post-exec fallback summary"
            );
            summary_prefix
        }
    };
    let artifacts = collect_fallback_result_artifacts(project_root, &session.meta_session_id);
    let completed_at = chrono::Utc::now();
    let fallback_result = SessionResult {
        post_exec_gate: None,
        status: "failure".to_string(),
        exit_code: 1,
        summary,
        tool: tool_name.to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: execution_start_time,
        completed_at,
        events_count: 0,
        artifacts,
        peak_memory_mb: None,
        fallback_chain: None,
        gate_timeout: false,
        warnings: Vec::new(),
        raw_process_exit_code: None,
        uncommitted_changes: None,
        manager_fields: Default::default(),
    };

    if let Err(save_err) = save_result(project_root, &session.meta_session_id, &fallback_result) {
        warn!(
            session = %session.meta_session_id,
            error = %save_err,
            "Failed to write fallback post-exec result.toml"
        );
        return;
    }
    csa_session::write_cooldown_marker_for_project(
        project_root,
        &session.meta_session_id,
        completed_at,
    );

    retire_post_exec_fallback_session(session, completed_at, Some("post_exec_error"));
}

fn retire_post_exec_fallback_session(
    session: &mut MetaSessionState,
    completed_at: chrono::DateTime<chrono::Utc>,
    termination_reason: Option<&str>,
) {
    if let Some(reason) = termination_reason {
        session.termination_reason = Some(reason.to_string());
    }
    session.last_accessed = completed_at;
    if session.phase != SessionPhase::Retired
        && let Err(phase_err) = session.apply_phase_event(PhaseEvent::Retired)
    {
        warn!(
            session = %session.meta_session_id,
            error = %phase_err,
            "Failed to transition post-exec fallback session to Retired; forcing terminal phase"
        );
        session.phase = SessionPhase::Retired;
    }
    if let Err(save_err) = save_session(session) {
        warn!(
            session = %session.meta_session_id,
            error = %save_err,
            "Failed to persist session state after fallback post-exec result write"
        );
    }
}

pub(super) fn read_output_log_tail(session_dir: &Path, max_lines: usize) -> Option<String> {
    let output_log_path = session_dir.join("output.log");
    let mut file = match File::open(&output_log_path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            warn!(
                path = %output_log_path.display(),
                error = %err,
                "Failed to read output.log for fallback summary"
            );
            return None;
        }
    };

    let file_len = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(err) => {
            warn!(
                path = %output_log_path.display(),
                error = %err,
                "Failed to stat output.log for fallback summary"
            );
            return None;
        }
    };
    let tail_start = file_len.saturating_sub(OUTPUT_LOG_TAIL_READ_BYTES);
    if let Err(err) = file.seek(SeekFrom::Start(tail_start)) {
        warn!(
            path = %output_log_path.display(),
            error = %err,
            "Failed to seek output.log for fallback summary"
        );
        return None;
    }

    let mut raw_bytes = Vec::new();
    match file.read_to_end(&mut raw_bytes) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            warn!(
                path = %output_log_path.display(),
                error = %err,
                "Failed to read output.log for fallback summary"
            );
            return None;
        }
    };

    let mut contents = String::from_utf8_lossy(&raw_bytes).into_owned();
    if tail_start > 0
        && let Some(first_newline) = contents.find('\n')
    {
        contents.drain(..=first_newline);
    }

    let tail = contents
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(max_lines)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if tail.is_empty() {
        return None;
    }

    Some(tail.into_iter().rev().collect::<Vec<_>>().join("\n"))
}
