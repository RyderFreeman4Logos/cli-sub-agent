use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_session::{Finding, FindingsFile, Severity, SeveritySummary};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct PersistedReviewArtifact {
    #[serde(default)]
    pub(super) findings: Vec<Finding>,
    #[serde(default)]
    pub(super) severity_summary: SeveritySummary,
    #[serde(default)]
    pub(super) overall_risk: Option<String>,
}

impl PersistedReviewArtifact {
    pub(super) fn overall_risk_is_severe(&self) -> bool {
        self.overall_risk.as_deref().is_some_and(|risk| {
            risk.eq_ignore_ascii_case("high") || risk.eq_ignore_ascii_case("critical")
        })
    }
}

pub(super) fn load_review_artifact_from_output(
    session_dir: &Path,
) -> Result<Option<PersistedReviewArtifact>, anyhow::Error> {
    let findings_path = session_dir.join("review-findings.json");
    if !findings_path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&findings_path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", findings_path.display()))?;
    let artifact = serde_json::from_str::<PersistedReviewArtifact>(&contents)
        .map_err(|error| anyhow::anyhow!("parse {}: {error}", findings_path.display()))?;
    Ok(Some(artifact))
}

pub(super) fn load_findings_toml_from_output(
    session_dir: &Path,
) -> Result<Option<FindingsFile>, anyhow::Error> {
    let findings_path = session_dir.join("output").join("findings.toml");
    if !findings_path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&findings_path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", findings_path.display()))?;
    let artifact = toml::from_str::<FindingsFile>(&contents)
        .map_err(|error| anyhow::anyhow!("parse {}: {error}", findings_path.display()))?;
    Ok(Some(artifact))
}

pub(super) fn severity_counts_for_artifact(
    artifact: &PersistedReviewArtifact,
    zero_severity_counts: impl Fn() -> std::collections::BTreeMap<Severity, u32>,
) -> std::collections::BTreeMap<Severity, u32> {
    let counts = [
        (Severity::Critical, artifact.severity_summary.critical),
        (Severity::High, artifact.severity_summary.high),
        (Severity::Medium, artifact.severity_summary.medium),
        (Severity::Low, artifact.severity_summary.low),
    ]
    .into_iter()
    .collect::<std::collections::BTreeMap<_, _>>();
    let total = counts.values().copied().sum::<u32>();
    if total == 0 && !artifact.findings.is_empty() {
        let mut recomputed = zero_severity_counts();
        for finding in &artifact.findings {
            *recomputed.entry(finding.severity.clone()).or_insert(0) += 1;
        }
        return recomputed;
    }
    counts
}

pub(super) fn severity_counts_for_findings_toml(
    artifact: &FindingsFile,
    zero_severity_counts: impl Fn() -> std::collections::BTreeMap<Severity, u32>,
) -> std::collections::BTreeMap<Severity, u32> {
    let mut counts = zero_severity_counts();
    for finding in &artifact.findings {
        *counts.entry(finding.severity.clone()).or_insert(0) += 1;
    }
    counts
}
