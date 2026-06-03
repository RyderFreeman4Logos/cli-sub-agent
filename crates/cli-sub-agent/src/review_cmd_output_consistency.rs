use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_core::types::ReviewDecision;
use csa_session::{FindingsFile, ReviewVerdictArtifact, Severity, write_findings_toml};

use super::artifacts::severity_counts_are_zero;
use super::artifacts::{has_blocking_severity, load_findings_toml_from_output};
use super::prose_signals::{reconcile_counts_with_prose, review_prose_signals};
use crate::review_cmd::prose_findings::severity_counts_from_review_findings;

const PROSE_FINDINGS_UNPARSED_REASON: &str = "prose_findings_present_but_unparsed";

pub(super) fn enforce_final_verdict_consistency(
    session_dir: &Path,
    artifact: &mut ReviewVerdictArtifact,
) -> Result<(), anyhow::Error> {
    let prose_signals = review_prose_signals(session_dir)?;
    let findings_file = load_findings_toml_from_output(session_dir)?.unwrap_or_default();
    let findings_file = if findings_file.findings.is_empty() && !prose_signals.findings.is_empty() {
        let findings_file = FindingsFile {
            findings: prose_signals.findings.clone(),
        };
        write_findings_toml(session_dir, &findings_file)
            .map_err(|error| anyhow::anyhow!("write prose-derived findings.toml: {error}"))?;
        let marker_path = session_dir
            .join("output")
            .join(super::super::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER);
        let _ = fs::remove_file(marker_path);
        findings_file
    } else {
        findings_file
    };

    let findings_counts = severity_counts_from_review_findings(&findings_file.findings);
    artifact.severity_counts =
        reconcile_counts_with_prose(artifact.severity_counts.clone(), &findings_counts);
    artifact.severity_counts = reconcile_counts_with_prose(
        artifact.severity_counts.clone(),
        &prose_signals.severity_counts,
    );

    let resume_to_fix = has_resume_to_fix_suggestion(session_dir)?;
    let blocking_prose =
        prose_signals.blocking_summary || has_blocking_severity(&prose_signals.severity_counts);
    let has_empty_machine_findings =
        findings_file.findings.is_empty() && severity_counts_are_zero(&artifact.severity_counts);
    let unparsed_actionable_prose =
        prose_signals.actionable_prose_sections && has_empty_machine_findings;

    if unparsed_actionable_prose {
        artifact
            .failure_reason
            .get_or_insert_with(|| PROSE_FINDINGS_UNPARSED_REASON.to_string());
    }

    if artifact.decision == ReviewDecision::Pass
        && (resume_to_fix || blocking_prose || unparsed_actionable_prose)
    {
        ensure_nonzero_fail_closed_count(&mut artifact.severity_counts);
        artifact.decision = ReviewDecision::Fail;
        artifact.verdict_legacy = "HAS_ISSUES".to_string();
    }

    if artifact.decision == ReviewDecision::Fail && has_empty_machine_findings {
        ensure_nonzero_fail_closed_count(&mut artifact.severity_counts);
    }

    Ok(())
}

fn ensure_nonzero_fail_closed_count(
    severity_counts: &mut std::collections::BTreeMap<Severity, u32>,
) {
    if severity_counts_are_zero(severity_counts) {
        *severity_counts.entry(Severity::Medium).or_insert(0) += 1;
    }
}

fn has_resume_to_fix_suggestion(session_dir: &Path) -> Result<bool, anyhow::Error> {
    let suggestion_path = session_dir.join("output").join("suggestion.toml");
    if !suggestion_path.exists() {
        return Ok(false);
    }
    let contents = fs::read_to_string(&suggestion_path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", suggestion_path.display()))?;
    let value = toml::from_str::<toml::Value>(&contents)
        .map_err(|error| anyhow::anyhow!("parse {}: {error}", suggestion_path.display()))?;
    Ok(value
        .get("suggestion")
        .and_then(|suggestion| suggestion.get("action"))
        .and_then(toml::Value::as_str)
        == Some("resume_to_fix"))
}
