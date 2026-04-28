use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use csa_core::env::CSA_SESSION_DIR_ENV_KEY;
use csa_core::types::ReviewDecision;
use csa_session::review_artifact::{Finding, ReviewArtifact};
use csa_session::{
    FindingsFile, ReviewFinding, ReviewFindingFileRange, ReviewVerdictArtifact,
    write_findings_toml, write_review_verdict,
};

use crate::review_consensus::{build_consolidated_artifact, write_consolidated_artifact};

use super::output::ReviewerOutcome;

fn load_multi_reviewer_artifacts(
    output_dir: &Path,
    reviewers: usize,
) -> Result<Vec<ReviewArtifact>> {
    let mut reviewer_artifacts = Vec::new();
    for reviewer_index in 1..=reviewers {
        let artifact_path = output_dir
            .join(format!("reviewer-{reviewer_index}"))
            .join("review-findings.json");

        if !artifact_path.exists() {
            continue;
        }

        let content = fs::read_to_string(&artifact_path)
            .with_context(|| format!("failed to read {}", artifact_path.display()))?;
        let artifact: ReviewArtifact = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", artifact_path.display()))?;
        reviewer_artifacts.push(artifact);
    }
    Ok(reviewer_artifacts)
}

pub(super) fn write_multi_reviewer_parent_artifacts(
    reviewers: usize,
    outcomes: &[ReviewerOutcome],
    final_verdict: &str,
    all_reviewers_unavailable: bool,
) -> Result<()> {
    let Some(session_dir) = std::env::var_os(CSA_SESSION_DIR_ENV_KEY) else {
        return Ok(());
    };
    let session_dir = PathBuf::from(session_dir);
    let session_id = std::env::var("CSA_SESSION_ID").unwrap_or_else(|_| "unknown".to_string());
    let reviewer_artifacts = load_multi_reviewer_artifacts(&session_dir, reviewers)?;
    let consolidated = build_consolidated_artifact(reviewer_artifacts, &session_id);
    write_consolidated_artifact(&consolidated, &session_dir)?;
    write_parent_findings_toml(&session_dir, &consolidated)?;
    write_parent_review_verdict(
        &session_dir,
        &session_id,
        &consolidated,
        final_verdict,
        all_reviewers_unavailable,
    )?;
    write_parent_review_summary(&session_dir, outcomes, final_verdict)?;
    write_parent_review_details(&session_dir, outcomes)?;
    Ok(())
}

fn write_parent_findings_toml(session_dir: &Path, artifact: &ReviewArtifact) -> Result<()> {
    let findings = artifact
        .findings
        .iter()
        .map(review_artifact_finding_to_findings_toml)
        .collect();
    write_findings_toml(session_dir, &FindingsFile { findings })
        .context("failed to write parent output/findings.toml")
}

fn review_artifact_finding_to_findings_toml(finding: &Finding) -> ReviewFinding {
    let file_ranges = finding
        .line
        .map(|line| {
            vec![ReviewFindingFileRange {
                path: finding.file.clone(),
                start: line,
                end: None,
            }]
        })
        .unwrap_or_default();
    ReviewFinding {
        id: finding.fid.clone(),
        severity: finding.severity.clone(),
        file_ranges,
        is_regression_of_commit: None,
        suggested_test_scenario: None,
        description: format!("{}: {}", finding.rule_id, finding.summary),
    }
}

fn write_parent_review_verdict(
    session_dir: &Path,
    session_id: &str,
    artifact: &ReviewArtifact,
    final_verdict: &str,
    all_reviewers_unavailable: bool,
) -> Result<()> {
    let decision = if all_reviewers_unavailable {
        ReviewDecision::Unavailable
    } else {
        ReviewDecision::from_str(final_verdict).unwrap_or(ReviewDecision::Uncertain)
    };
    let verdict = ReviewVerdictArtifact::from_parts(
        session_id.to_string(),
        decision,
        final_verdict.to_string(),
        &artifact.findings,
        Vec::new(),
    );
    write_review_verdict(session_dir, &verdict)
        .context("failed to write parent output/review-verdict.json")
}

fn write_parent_review_summary(
    session_dir: &Path,
    outcomes: &[ReviewerOutcome],
    final_verdict: &str,
) -> Result<()> {
    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let mut summary = format!("Final verdict: {final_verdict}\n\nReviewer outcomes:\n");
    for outcome in outcomes {
        summary.push_str(&format!(
            "- reviewer {} ({}) => {}",
            outcome.reviewer_index + 1,
            outcome.tool,
            outcome.verdict
        ));
        if let Some(diagnostic) = &outcome.diagnostic {
            summary.push_str(&format!("; diagnostic: {diagnostic}"));
        }
        summary.push('\n');
    }
    fs::write(output_dir.join("summary.md"), summary)
        .context("failed to write parent output/summary.md")
}

fn write_parent_review_details(session_dir: &Path, outcomes: &[ReviewerOutcome]) -> Result<()> {
    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let mut details = String::new();
    for outcome in outcomes {
        details.push_str(&format!(
            "## Reviewer {} ({})\n\nVerdict: {}\nExit code: {}\n",
            outcome.reviewer_index + 1,
            outcome.tool,
            outcome.verdict,
            outcome.exit_code
        ));
        if let Some(diagnostic) = &outcome.diagnostic {
            details.push_str(&format!("Diagnostic: {diagnostic}\n"));
        }
        details.push('\n');
        details.push_str(&outcome.output);
        if !details.ends_with('\n') {
            details.push('\n');
        }
        details.push('\n');
    }
    fs::write(output_dir.join("details.md"), details)
        .context("failed to write parent output/details.md")
}

#[cfg(test)]
mod tests {
    use super::write_multi_reviewer_parent_artifacts;
    use crate::review_consensus::UNAVAILABLE;
    use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
    use csa_core::env::CSA_SESSION_DIR_ENV_KEY;
    use csa_core::types::{ReviewDecision, ToolName};
    use csa_session::review_artifact::{
        Finding, FindingsFile, ReviewArtifact, ReviewVerdictArtifact, Severity, SeveritySummary,
    };
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn write_multi_reviewer_parent_artifacts_writes_output_sidecars() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let temp = tempdir().expect("tempdir should be created");
        let session_dir = temp.path().display().to_string();
        let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
        let _session_id_guard =
            ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");

        let reviewer_dir = temp.path().join("reviewer-1");
        fs::create_dir_all(&reviewer_dir).expect("reviewer dir should be created");
        let findings = vec![Finding {
            severity: Severity::High,
            fid: "FID-1".to_string(),
            file: "src/lib.rs".to_string(),
            line: Some(7),
            rule_id: "rule.review.parent-sidecars".to_string(),
            summary: "parent sidecar finding".to_string(),
            engine: "reviewer".to_string(),
        }];
        let artifact = ReviewArtifact {
            severity_summary: SeveritySummary::from_findings(&findings),
            findings,
            review_mode: Some("diff".to_string()),
            schema_version: "1.0".to_string(),
            session_id: "01CHILDSESSION0000000000000".to_string(),
            timestamp: chrono::Utc::now(),
        };
        fs::write(
            reviewer_dir.join("review-findings.json"),
            serde_json::to_vec_pretty(&artifact).expect("artifact should serialize"),
        )
        .expect("review artifact should be written");

        let outcomes = vec![
            super::super::output::ReviewerOutcome {
                reviewer_index: 0,
                tool: ToolName::Codex,
                session_id: "01CHILDSESSION0000000000000".to_string(),
                output: "Reviewer details".to_string(),
                exit_code: 1,
                verdict: crate::review_consensus::HAS_ISSUES,
                diagnostic: None,
            },
            super::super::output::ReviewerOutcome {
                reviewer_index: 1,
                tool: ToolName::GeminiCli,
                session_id: "reviewer-2-unavailable".to_string(),
                output: "Review unavailable: reviewer timed out after 1800s\n".to_string(),
                exit_code: 1,
                verdict: UNAVAILABLE,
                diagnostic: Some("reviewer timed out after 1800s".to_string()),
            },
        ];

        write_multi_reviewer_parent_artifacts(
            2,
            &outcomes,
            crate::review_consensus::HAS_ISSUES,
            false,
        )
        .expect("parent artifacts should be produced");

        let output_dir = temp.path().join("output");
        let findings_toml: FindingsFile = toml::from_str(
            &fs::read_to_string(output_dir.join("findings.toml"))
                .expect("findings.toml should exist"),
        )
        .expect("findings.toml should parse");
        assert_eq!(findings_toml.findings.len(), 1);
        assert_eq!(findings_toml.findings[0].id, "FID-1");

        let verdict: ReviewVerdictArtifact = serde_json::from_str(
            &fs::read_to_string(output_dir.join("review-verdict.json"))
                .expect("review-verdict.json should exist"),
        )
        .expect("review verdict should parse");
        assert_eq!(verdict.decision, ReviewDecision::Fail);
        assert_eq!(verdict.severity_counts[&Severity::High], 1);
        assert!(
            fs::read_to_string(output_dir.join("summary.md"))
                .expect("summary should exist")
                .contains("reviewer 2 (gemini-cli) => UNAVAILABLE")
        );
        assert!(
            fs::read_to_string(output_dir.join("details.md"))
                .expect("details should exist")
                .contains("Review unavailable: reviewer timed out")
        );
    }

    #[test]
    fn write_multi_reviewer_parent_artifacts_marks_all_unavailable() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let temp = tempdir().expect("tempdir should be created");
        let session_dir = temp.path().display().to_string();
        let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
        let _session_id_guard =
            ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
        let outcomes = vec![super::super::output::ReviewerOutcome {
            reviewer_index: 0,
            tool: ToolName::Codex,
            session_id: "reviewer-1-unavailable".to_string(),
            output: "Review unavailable: reviewer timed out after 1800s\n".to_string(),
            exit_code: 1,
            verdict: UNAVAILABLE,
            diagnostic: Some("reviewer timed out after 1800s".to_string()),
        }];

        write_multi_reviewer_parent_artifacts(1, &outcomes, UNAVAILABLE, true)
            .expect("parent artifacts should be produced");

        let verdict: ReviewVerdictArtifact = serde_json::from_str(
            &fs::read_to_string(temp.path().join("output").join("review-verdict.json"))
                .expect("review-verdict.json should exist"),
        )
        .expect("review verdict should parse");
        assert_eq!(verdict.decision, ReviewDecision::Unavailable);
        assert_eq!(verdict.verdict_legacy, UNAVAILABLE);
    }
}
