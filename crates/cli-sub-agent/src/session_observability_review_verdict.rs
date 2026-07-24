use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_core::types::ReviewDecision;
use csa_session::{ReviewSessionMeta, ReviewVerdictArtifact, SessionResult};

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

pub(crate) fn sync_clean_pass_result_status_from_sidecars(
    session_dir: &Path,
    result: &mut SessionResult,
) -> Result<bool> {
    if result.post_exec_gate.is_some()
        || crate::session_observability::require_commit_contract_failed(result)
    {
        return Ok(false);
    }
    let Some(artifact) = read_review_verdict_artifact(session_dir)? else {
        return Ok(false);
    };
    if read_review_meta(session_dir)?.is_some_and(|meta| !meta_allows_clean_pass(&meta)) {
        return Ok(false);
    }
    if artifact.decision != ReviewDecision::Pass
        || !result_has_clean_review_summary(session_dir, result)
    {
        return Ok(false);
    }
    Ok(sync_result_exit_code(result, 0))
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
    let artifact = read_review_verdict_artifact(session_dir)?;
    let review_meta = read_review_meta(session_dir)?;
    if review_meta
        .as_ref()
        .is_some_and(|meta| !meta_allows_clean_pass(meta))
    {
        return Ok(Some(1));
    }

    Ok(artifact.map(|artifact| {
        crate::verdict_exit_code::exit_code_from_review_decision(artifact.decision)
    }))
}

fn read_review_verdict_artifact(session_dir: &Path) -> Result<Option<ReviewVerdictArtifact>> {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if !verdict_path.is_file() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&verdict_path)?;
    serde_json::from_str(&raw).map(Some).map_err(Into::into)
}

fn read_review_meta(session_dir: &Path) -> Result<Option<ReviewSessionMeta>> {
    let meta_path = session_dir.join("review_meta.json");
    if !meta_path.is_file() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&meta_path)?;
    serde_json::from_str(&raw).map(Some).map_err(Into::into)
}

fn meta_allows_clean_pass(meta: &ReviewSessionMeta) -> bool {
    matches!(meta.decision.parse(), Ok(ReviewDecision::Pass))
        && meta.exit_code == 0
        && !meta.requires_fail_closed_verdict()
        && meta.fix_clean_converged()
}

fn result_has_clean_review_summary(session_dir: &Path, result: &SessionResult) -> bool {
    let Some(summary) =
        crate::session_summary_text::human_session_summary(session_dir, &result.summary)
    else {
        return false;
    };
    let lower = summary.to_ascii_lowercase();
    [
        "no blocking",
        "no blockers",
        "no actionable findings",
        "no issues found",
        "no issues were found",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
        || crate::review_cmd::detect_bounded_clean_verdict_token(&summary)
}
