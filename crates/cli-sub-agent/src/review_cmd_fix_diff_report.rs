use std::path::Path;

#[cfg(test)]
use csa_core::types::ReviewDecision;
use csa_session::state::ReviewSessionMeta;

use super::super::{diff_size, output};

#[cfg(test)]
pub(super) fn empty_diff_report() -> diff_size::ReviewDiffReport<'static> {
    diff_size::ReviewDiffReport {
        diff_size: None,
        large_diff_warning: None,
    }
}

pub(super) fn persist_fix_review_meta(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
    diff_report: diff_size::ReviewDiffReport<'_>,
) {
    diff_size::persist_review_meta_with_diff_report(
        project_root,
        review_meta,
        diff_report.diff_size,
        diff_report.large_diff_warning,
    );
}

pub(super) fn persist_fix_review_verdict(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
    diff_report: diff_size::ReviewDiffReport<'_>,
) {
    let Some(mut artifact) =
        output::persist_review_verdict_artifact(project_root, review_meta, &[], Vec::new())
    else {
        return;
    };
    diff_size::persist_review_verdict_diff_report(
        project_root,
        &review_meta.session_id,
        &mut artifact,
        diff_report.diff_size,
        diff_report.large_diff_warning,
    );
}

#[cfg(test)]
pub(crate) fn persist_fix_final_artifacts_for_tests(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
    quality_gate_passed: bool,
) -> ReviewDecision {
    super::persist_fix_final_artifacts(project_root, review_meta, quality_gate_passed)
}

#[cfg(test)]
pub(crate) fn persist_fix_final_artifacts_for_tests_with_output(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
    quality_gate_passed: bool,
    current_fix_output: &str,
) -> ReviewDecision {
    super::persist_fix_final_artifacts_with_current_output(
        project_root,
        review_meta,
        quality_gate_passed,
        Some(current_fix_output),
        empty_diff_report(),
    )
}

#[cfg(test)]
pub(crate) fn persist_fix_final_artifacts_for_tests_with_output_and_diff_report(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
    quality_gate_passed: bool,
    current_fix_output: &str,
    diff_report: diff_size::ReviewDiffReport<'_>,
) -> ReviewDecision {
    super::persist_fix_final_artifacts_with_current_output(
        project_root,
        review_meta,
        quality_gate_passed,
        Some(current_fix_output),
        diff_report,
    )
}
