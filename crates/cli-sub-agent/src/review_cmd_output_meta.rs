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
    if artifact.decision == ReviewDecision::Pass {
        final_meta.status_reason = None;
        final_meta.primary_failure = None;
        final_meta.failure_reason = None;
    }
    final_meta
}
