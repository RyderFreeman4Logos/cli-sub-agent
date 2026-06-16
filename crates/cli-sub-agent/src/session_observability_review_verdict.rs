use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_session::{ReviewVerdictArtifact, SessionResult};

pub(super) fn sync_review_verdict_exit_code(
    session_dir: &Path,
    result: &mut SessionResult,
    force_review_failure: bool,
) -> Result<bool> {
    let exit_code = if force_review_failure {
        Some(1)
    } else {
        read_review_verdict_exit_code(session_dir)?
    };
    let Some(exit_code) = exit_code else {
        return Ok(false);
    };
    Ok(sync_result_exit_code(result, exit_code))
}

fn sync_result_exit_code(result: &mut SessionResult, exit_code: i32) -> bool {
    let status = SessionResult::status_from_exit_code(exit_code);
    if result.exit_code == exit_code && result.status == status {
        return false;
    }

    result.exit_code = exit_code;
    result.status = status;
    true
}

fn read_review_verdict_exit_code(session_dir: &Path) -> Result<Option<i32>> {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if !verdict_path.is_file() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&verdict_path)?;
    let artifact: ReviewVerdictArtifact = serde_json::from_str(&raw)?;
    Ok(Some(
        crate::verdict_exit_code::exit_code_from_review_decision(artifact.decision),
    ))
}
