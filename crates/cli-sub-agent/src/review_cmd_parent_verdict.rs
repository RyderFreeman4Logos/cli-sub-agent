use std::path::Path;

use anyhow::{Context, Result};
use csa_core::types::ReviewDecision;
use csa_session::review_artifact::Finding;
use csa_session::{ReviewVerdictArtifact, write_review_verdict};

use super::super::diff_size::{ReviewDiffReport, apply_large_diff_warning};

pub(super) fn write_parent_review_verdict(
    session_dir: &Path,
    session_id: &str,
    severity_count_findings: &[Finding],
    decision: ReviewDecision,
    verdict_legacy: &str,
    diff_report: ReviewDiffReport<'_>,
    review_mode: Option<&str>,
) -> Result<()> {
    let mut verdict = ReviewVerdictArtifact::from_parts(
        session_id.to_string(),
        decision,
        verdict_legacy.to_string(),
        severity_count_findings,
        Vec::new(),
    );
    verdict.review_mode = review_mode.map(str::to_string);
    verdict.diff_size = diff_report.diff_size.cloned();
    apply_large_diff_warning(&mut verdict, diff_report.large_diff_warning);
    write_review_verdict(session_dir, &verdict)
        .context("failed to write parent output/review-verdict.json")
}
