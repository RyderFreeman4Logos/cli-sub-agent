use std::{fs, path::Path};

use anyhow::{Context, Result};
use csa_session::review_artifact::{Finding, FindingsFile, ReviewArtifact};

use crate::bug_class::{CONSOLIDATED_REVIEW_ARTIFACT_FILE, SINGLE_REVIEW_ARTIFACT_FILE};

const FINDINGS_TOML_ENGINE: &str = "findings.toml";
const FINDINGS_TOML_RULE_ID: &str = "review.findings_toml";

/// Read review findings for a session, preferring the per-round authoritative
/// `output/findings.toml` artifact and falling back to legacy root JSON
/// artifacts for backward compatibility.
pub(crate) fn read_session_findings_or_fall_back(
    session_dir: &Path,
) -> Result<Option<Vec<Finding>>> {
    if let Some(findings) = read_session_findings_toml(session_dir)? {
        return Ok(Some(findings));
    }

    read_legacy_review_artifact(session_dir)
}

fn read_session_findings_toml(session_dir: &Path) -> Result<Option<Vec<Finding>>> {
    let findings_path = session_dir.join("output").join("findings.toml");
    if !findings_path.is_file() {
        return Ok(None);
    }

    let content = fs::read_to_string(&findings_path)
        .with_context(|| format!("failed to read {}", findings_path.display()))?;
    let artifact: FindingsFile = toml::from_str(&content)
        .with_context(|| format!("failed to parse {}", findings_path.display()))?;
    Ok(Some(
        artifact
            .findings
            .into_iter()
            .map(|finding| {
                let primary_range = finding.file_ranges.first();
                Finding {
                    severity: finding.severity,
                    fid: finding.id,
                    file: primary_range
                        .map(|range| range.path.clone())
                        .unwrap_or_else(|| "<unknown>".to_string()),
                    line: primary_range.map(|range| range.start),
                    rule_id: FINDINGS_TOML_RULE_ID.to_string(),
                    summary: finding.description,
                    engine: FINDINGS_TOML_ENGINE.to_string(),
                }
            })
            .collect(),
    ))
}

fn read_legacy_review_artifact(session_dir: &Path) -> Result<Option<Vec<Finding>>> {
    for artifact_file in [
        CONSOLIDATED_REVIEW_ARTIFACT_FILE,
        SINGLE_REVIEW_ARTIFACT_FILE,
    ] {
        let artifact_path = session_dir.join(artifact_file);
        if !artifact_path.is_file() {
            continue;
        }

        let content = fs::read_to_string(&artifact_path)
            .with_context(|| format!("failed to read {}", artifact_path.display()))?;
        let artifact: ReviewArtifact = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", artifact_path.display()))?;
        return Ok(Some(artifact.findings));
    }

    Ok(None)
}
