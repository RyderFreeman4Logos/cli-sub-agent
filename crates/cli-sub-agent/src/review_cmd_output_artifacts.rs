use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use csa_session::{Finding, FindingsFile, Severity, SeveritySummary};
use serde::Deserialize;
use tracing::warn;

use crate::bug_class::{CONSOLIDATED_REVIEW_ARTIFACT_FILE, SINGLE_REVIEW_ARTIFACT_FILE};
use crate::review_cmd::artifact_parse::parse_review_artifact_fields_lossy;

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
    let Some(findings_path) = review_artifact_path(session_dir) else {
        return Ok(None);
    };

    let contents = fs::read_to_string(&findings_path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", findings_path.display()))?;
    // Gracefully handle parse failures (e.g. LLM emits severity="info" which is not in
    // the Severity enum). The file EXISTS — the review ran and produced output — so a
    // parse error must not cascade to Err (which triggers the persist_review_verdict
    // fallback that blindly copies meta.decision=Fail). Instead, treat the file as
    // present but containing no parseable findings: return an empty artifact so the
    // downstream zero-counts guard (added in #1349) can fire correctly. (#1352)
    match serde_json::from_str::<PersistedReviewArtifact>(&contents) {
        Ok(artifact) => Ok(Some(artifact)),
        Err(error) => {
            if let Ok(fields) = parse_review_artifact_fields_lossy(&contents) {
                warn!(
                    path = %findings_path.display(),
                    error = %error,
                    "Parsed review artifact JSON lossily; ignored findings with unsupported severities"
                );
                return Ok(Some(PersistedReviewArtifact {
                    findings: fields.findings,
                    severity_summary: fields.severity_summary,
                    overall_risk: fields.overall_risk,
                }));
            }
            warn!(
                path = %findings_path.display(),
                error = %error,
                "Failed to parse review artifact JSON; treating as zero-findings"
            );
            Ok(Some(PersistedReviewArtifact {
                findings: Vec::new(),
                severity_summary: SeveritySummary::default(),
                overall_risk: None,
            }))
        }
    }
}

fn review_artifact_path(session_dir: &Path) -> Option<PathBuf> {
    [
        CONSOLIDATED_REVIEW_ARTIFACT_FILE,
        SINGLE_REVIEW_ARTIFACT_FILE,
    ]
    .into_iter()
    .map(|artifact_file| session_dir.join(artifact_file))
    .find(|artifact_path| artifact_path.is_file())
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
    match toml::from_str::<FindingsFile>(&contents) {
        Ok(artifact) => Ok(Some(artifact)),
        Err(error) => {
            warn!(
                path = %findings_path.display(),
                error = %error,
                "Failed to parse findings.toml; treating as absent"
            );
            Ok(None)
        }
    }
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

/// Load severity counts from persisted review JSON when present.
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
