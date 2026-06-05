use std::path::Path;
use std::str::FromStr;

use csa_core::types::ReviewDecision;
use csa_session::state::ReviewSessionMeta;
use csa_session::{Finding, ReviewVerdictArtifact};

pub(in crate::review_cmd) fn fail_closed_review_meta(
    project_root: &Path,
    meta: &ReviewSessionMeta,
) -> ReviewSessionMeta {
    if csa_session::get_session_dir(project_root, &meta.session_id).is_err() {
        return meta.clone();
    }
    if !meta.requires_fail_closed_verdict() {
        return meta.clone();
    }

    let mut closed = meta.clone();
    let decision = fail_closed_decision_for_meta(meta);
    closed.decision = decision.as_str().to_string();
    closed.verdict = fail_closed_legacy_verdict(decision).to_string();
    if closed.exit_code == 0 {
        closed.exit_code = crate::verdict_exit_code::exit_code_from_review_decision(decision);
    }
    closed
}

pub(super) fn fail_closed_review_verdict_artifact(
    meta: &ReviewSessionMeta,
    findings: &[Finding],
    prior_round_refs: Vec<String>,
) -> ReviewVerdictArtifact {
    let decision = fail_closed_decision_for_meta(meta);
    let mut artifact = ReviewVerdictArtifact::from_parts(
        meta.session_id.clone(),
        decision,
        fail_closed_legacy_verdict(decision),
        findings,
        prior_round_refs,
    );
    artifact.routed_to = meta.routed_to.clone();
    artifact.primary_failure = meta.primary_failure.clone();
    artifact.failure_reason = meta.failure_reason.clone();
    artifact.review_mode = meta.review_mode.clone();
    artifact
}

fn fail_closed_decision_for_meta(meta: &ReviewSessionMeta) -> ReviewDecision {
    match ReviewDecision::from_str(&meta.decision).unwrap_or(ReviewDecision::Unavailable) {
        ReviewDecision::Fail => ReviewDecision::Fail,
        ReviewDecision::Uncertain => ReviewDecision::Uncertain,
        ReviewDecision::Unavailable => ReviewDecision::Unavailable,
        ReviewDecision::Pass | ReviewDecision::Skip => ReviewDecision::Unavailable,
    }
}

fn fail_closed_legacy_verdict(decision: ReviewDecision) -> &'static str {
    match decision {
        ReviewDecision::Fail => "HAS_ISSUES",
        ReviewDecision::Uncertain => "UNCERTAIN",
        ReviewDecision::Unavailable | ReviewDecision::Pass | ReviewDecision::Skip => "UNAVAILABLE",
    }
}
