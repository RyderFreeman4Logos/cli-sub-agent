use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_session::{GATE_FAILURE_LOG_REL_PATH, PostExecGateReport, SessionArtifact, SessionResult};

const INFERRED_GATE_COMMAND: &str = "post-exec gate";

pub(super) fn infer_post_exec_gate_failure_from_log(
    session_dir: &Path,
    result_path: &Path,
    result: &mut SessionResult,
) -> Result<bool> {
    if result.post_exec_gate.is_some() || !result_failed(result) {
        return Ok(false);
    }
    let log_path = session_dir.join(GATE_FAILURE_LOG_REL_PATH);
    if !log_path.is_file() {
        return Ok(false);
    }
    if !gate_log_matches_current_failure(result, &log_path, result_path) {
        return Ok(false);
    }

    let raw = fs::read_to_string(&log_path)?;
    let redacted = csa_session::redact_text_content(&raw);
    let gate_exit_code = if result.exit_code == 0 {
        1
    } else {
        result.exit_code
    };
    let report = PostExecGateReport::from_redacted_gate_output(
        INFERRED_GATE_COMMAND,
        gate_exit_code,
        &redacted,
    );

    result.exit_code = gate_exit_code;
    result.status = SessionResult::status_from_exit_code(gate_exit_code);
    result.summary = csa_session::post_exec_gate_failure_summary(&report);
    result.post_exec_gate = Some(report);
    ensure_gate_failure_artifact(result);
    Ok(true)
}

fn gate_log_matches_current_failure(
    result: &SessionResult,
    log_path: &Path,
    result_path: &Path,
) -> bool {
    result_owns_gate_failure_log(result) || gate_log_is_fresh_for_result(log_path, result_path)
}

fn result_owns_gate_failure_log(result: &SessionResult) -> bool {
    result
        .artifacts
        .iter()
        .any(|artifact| artifact.path == GATE_FAILURE_LOG_REL_PATH && !artifact.display_only)
}

fn gate_log_is_fresh_for_result(log_path: &Path, result_path: &Path) -> bool {
    let Ok(log_modified) = fs::metadata(log_path).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    let Ok(result_modified) = fs::metadata(result_path).and_then(|metadata| metadata.modified())
    else {
        return false;
    };

    log_modified >= result_modified
}

fn result_failed(result: &SessionResult) -> bool {
    result.exit_code != 0 || !result.status.eq_ignore_ascii_case("success")
}

fn ensure_gate_failure_artifact(result: &mut SessionResult) {
    if let Some(artifact) = result
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.path == GATE_FAILURE_LOG_REL_PATH)
    {
        artifact.display_only = false;
        return;
    }
    result
        .artifacts
        .push(SessionArtifact::new(GATE_FAILURE_LOG_REL_PATH));
    result
        .artifacts
        .sort_by(|left, right| left.path.cmp(&right.path));
}
