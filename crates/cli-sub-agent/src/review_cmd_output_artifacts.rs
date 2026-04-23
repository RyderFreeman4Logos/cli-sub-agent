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

/// Load severity counts from review-findings.json when present.
///
/// Returns `Some(counts)` if JSON exists and has any non-zero counts (even
/// low-only). Returns `None` if JSON is absent, unparseable, or all-zero.
///
/// Used to preserve informational low-severity counts in the final verdict
/// when findings.toml is empty but JSON recorded low findings (#1048 M1).
pub(super) fn json_severity_counts_if_present(
    session_dir: &Path,
    zero_severity_counts: impl Fn() -> std::collections::BTreeMap<Severity, u32>,
) -> Result<Option<std::collections::BTreeMap<Severity, u32>>, anyhow::Error> {
    let Some(json_artifact) = load_review_artifact_from_output(session_dir)? else {
        return Ok(None);
    };
    let json_counts = severity_counts_for_artifact(&json_artifact, zero_severity_counts);
    if json_counts.values().all(|count| *count == 0) {
        return Ok(None);
    }
    Ok(Some(json_counts))
}

/// Whether the severity counts contain any blocking findings (critical, high, or medium).
pub(super) fn has_blocking_severity(counts: &std::collections::BTreeMap<Severity, u32>) -> bool {
    counts
        .iter()
        .any(|(severity, count)| *count > 0 && *severity > Severity::Low)
}

pub(super) fn severity_counts_are_zero(counts: &std::collections::BTreeMap<Severity, u32>) -> bool {
    counts.values().all(|count| *count == 0)
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
