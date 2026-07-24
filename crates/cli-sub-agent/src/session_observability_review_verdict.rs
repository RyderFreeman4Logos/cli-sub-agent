use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_core::types::ReviewDecision;
use csa_session::{ReviewSessionMeta, ReviewVerdictArtifact, SessionResult};

enum ReviewSidecarAuthority {
    Absent,
    LegacyCleanMetaWithoutArtifact,
    Invalid,
    Valid {
        meta: Box<ReviewSessionMeta>,
        artifact: Box<ReviewVerdictArtifact>,
    },
}

pub(super) fn sync_review_verdict_exit_code(
    session_dir: &Path,
    result: &mut SessionResult,
    force_review_failure: bool,
) -> Result<bool> {
    let authority = read_review_sidecar_authority(session_dir)?;
    let exit_code = match authority {
        ReviewSidecarAuthority::Absent | ReviewSidecarAuthority::LegacyCleanMetaWithoutArtifact
            if !force_review_failure =>
        {
            None
        }
        ReviewSidecarAuthority::Valid {
            ref meta,
            ref artifact,
        } if !force_review_failure && sidecars_allow_clean_pass(meta, artifact) => None,
        ReviewSidecarAuthority::Absent
        | ReviewSidecarAuthority::LegacyCleanMetaWithoutArtifact
        | ReviewSidecarAuthority::Invalid
        | ReviewSidecarAuthority::Valid { .. } => Some(1),
    };
    Ok(exit_code.is_some_and(|exit_code| sync_result_exit_code(result, exit_code)))
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
    let ReviewSidecarAuthority::Valid { meta, artifact } =
        read_review_sidecar_authority(session_dir)?
    else {
        return Ok(false);
    };
    if !sidecars_allow_clean_pass(&meta, &artifact)
        || artifact.timestamp < result.completed_at
        || !result_has_clean_review_summary(session_dir, result)
    {
        return Ok(false);
    }
    Ok(sync_result_exit_code(result, 0))
}

pub(crate) fn review_sidecars_allow_clean_pass(session_dir: &Path) -> Result<bool> {
    let authority = read_review_sidecar_authority(session_dir)?;
    Ok(matches!(
        authority,
        ReviewSidecarAuthority::Valid { ref meta, ref artifact }
            if sidecars_allow_clean_pass(meta, artifact)
    ))
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

fn read_review_sidecar_authority(session_dir: &Path) -> Result<ReviewSidecarAuthority> {
    let artifact = read_review_verdict_artifact(session_dir)?;
    let meta = read_review_meta(session_dir)?;
    match (meta, artifact) {
        (None, None) => Ok(ReviewSidecarAuthority::Absent),
        // Legacy review flows wrote clean metadata but no separate verdict artifact.
        // Treat only independently clean metadata as neutral; every non-pass,
        // incomplete, or malformed metadata result remains fail-closed.
        (Some(meta), None) if meta_allows_clean_pass(&meta) => {
            Ok(ReviewSidecarAuthority::LegacyCleanMetaWithoutArtifact)
        }
        (Some(meta), Some(artifact)) if sidecars_match(&meta, &artifact) => {
            Ok(ReviewSidecarAuthority::Valid {
                meta: Box::new(meta),
                artifact: Box::new(artifact),
            })
        }
        _ => Ok(ReviewSidecarAuthority::Invalid),
    }
}

fn sidecars_match(meta: &ReviewSessionMeta, artifact: &ReviewVerdictArtifact) -> bool {
    let Ok(decision) = meta.decision.parse::<ReviewDecision>() else {
        return false;
    };
    !meta.session_id.trim().is_empty()
        && meta.session_id == artifact.session_id
        && artifact.timestamp >= meta.timestamp
        && decision == artifact.decision
        && meta.verdict == artifact.verdict_legacy
        && artifact.review_iterations == Some(meta.review_iterations)
        && artifact.fix_rounds == Some(meta.fix_rounds)
}

fn sidecars_allow_clean_pass(meta: &ReviewSessionMeta, artifact: &ReviewVerdictArtifact) -> bool {
    meta_allows_clean_pass(meta)
        && artifact.decision == ReviewDecision::Pass
        && !artifact_has_severity_counts(artifact)
}

fn artifact_has_severity_counts(artifact: &ReviewVerdictArtifact) -> bool {
    artifact.severity_counts.values().any(|count| *count > 0)
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
