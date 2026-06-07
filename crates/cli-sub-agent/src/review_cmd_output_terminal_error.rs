use std::{fs, path::Path};

use anyhow::Result;
use csa_core::types::ReviewDecision;
use csa_session::{Finding, ReviewVerdictArtifact, state::ReviewSessionMeta};

use super::text::terminal_tool_error_reason;

fn unavailable_terminal_error_artifact(
    meta: &ReviewSessionMeta,
    findings: &[Finding],
    reason: String,
) -> ReviewVerdictArtifact {
    let mut artifact = ReviewVerdictArtifact::from_parts(
        meta.session_id.clone(),
        ReviewDecision::Unavailable,
        "UNAVAILABLE",
        findings,
        Vec::new(),
    );
    artifact.routed_to = meta.routed_to.clone();
    artifact.primary_failure = meta.primary_failure.clone();
    artifact.failure_reason = meta.failure_reason.clone().or(Some(reason));
    artifact.review_mode = meta.review_mode.clone();
    artifact
}

pub(in crate::review_cmd) fn terminal_error_artifact_from_full_output(
    session_dir: &Path,
    meta: &ReviewSessionMeta,
    findings: &[Finding],
) -> Result<Option<ReviewVerdictArtifact>> {
    let full_output_path = session_dir.join("output").join("full.md");
    if !full_output_path.exists() {
        return Ok(None);
    }
    let raw_output = fs::read_to_string(&full_output_path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", full_output_path.display()))?;
    Ok(terminal_tool_error_reason(&raw_output)
        .map(|reason| unavailable_terminal_error_artifact(meta, findings, reason)))
}
