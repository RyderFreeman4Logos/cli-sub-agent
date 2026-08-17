use csa_core::types::ReviewDecision;
use csa_session::ReviewVerdictArtifact;
use csa_session::state::ReviewSessionMeta;

pub(super) fn apply_review_meta_to_artifact(
    artifact: &mut ReviewVerdictArtifact,
    meta: &ReviewSessionMeta,
) {
    artifact.routed_to = meta.routed_to.clone();
    artifact.primary_failure = meta.primary_failure.clone();
    artifact.failure_reason = meta
        .failure_reason
        .clone()
        .or_else(|| meta.status_reason.clone())
        .or_else(|| artifact.failure_reason.take());
    artifact.review_mode = meta.review_mode.clone();
    artifact.review_iterations = Some(meta.review_iterations);
    artifact.fix_rounds = Some(meta.fix_rounds);
}

pub(in crate::review_cmd) fn review_meta_for_verdict_artifact(
    meta: &ReviewSessionMeta,
    artifact: &ReviewVerdictArtifact,
) -> ReviewSessionMeta {
    let mut final_meta = meta.clone();
    final_meta.decision = artifact.decision.as_str().to_string();
    final_meta.verdict = artifact.verdict_legacy.clone();
    final_meta.exit_code =
        crate::verdict_exit_code::exit_code_from_review_decision(artifact.decision);
    if meta.failure_reason.is_some()
        || artifact.failure_reason.as_deref() != meta.status_reason.as_deref()
    {
        final_meta.failure_reason = artifact.failure_reason.clone();
    }
    if super::consistency::artifact_failure_reason_is_placeholder(meta.status_reason.as_deref())
        && artifact.failure_reason.as_deref() != meta.status_reason.as_deref()
    {
        final_meta.status_reason = None;
    }
    if artifact.decision == ReviewDecision::Pass {
        final_meta.status_reason = None;
        final_meta.primary_failure = None;
        final_meta.failure_reason = None;
    }
    final_meta
}
