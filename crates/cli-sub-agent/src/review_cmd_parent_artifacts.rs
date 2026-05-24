use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use csa_core::env::CSA_SESSION_DIR_ENV_KEY;
use csa_core::types::ReviewDecision;
use csa_session::review_artifact::{Finding, ReviewArtifact, Severity};
use csa_session::state::{ReviewSessionMeta, write_review_meta};
use csa_session::{
    FindingsFile, ReviewFinding, ReviewFindingFileRange, ReviewVerdictArtifact,
    write_findings_toml, write_review_verdict,
};
use serde::Deserialize;

use crate::review_consensus::{
    CLEAN, HAS_ISSUES, SKIP, UNAVAILABLE, build_consolidated_artifact, write_consolidated_artifact,
};

use super::output::ReviewerOutcome;

pub(super) struct MultiReviewerConsensusArtifacts<'a> {
    pub(super) project_root: &'a Path,
    pub(super) reviewers: usize,
    pub(super) outcomes: &'a [ReviewerOutcome],
    pub(super) final_verdict: &'a str,
    pub(super) all_reviewers_unavailable: bool,
    pub(super) head_sha: &'a str,
    pub(super) scope: &'a str,
    pub(super) review_iterations: u32,
    pub(super) diff_fingerprint: Option<String>,
}

const CSA_DAEMON_SESSION_DIR_ENV_KEY: &str = "CSA_DAEMON_SESSION_DIR";
const CSA_DAEMON_SESSION_ID_ENV_KEY: &str = "CSA_DAEMON_SESSION_ID";
const CSA_SESSION_ID_ENV_KEY: &str = "CSA_SESSION_ID";

#[derive(Debug, Deserialize)]
struct ReviewerFindingsContractArtifact {
    #[serde(default)]
    verdict: Option<String>,
    #[serde(default)]
    findings: Vec<Finding>,
    #[serde(default)]
    summary: Option<String>,
}

fn parse_reviewer_artifact(path: &Path, content: &str) -> Result<ReviewArtifact> {
    if let Ok(artifact) = serde_json::from_str::<ReviewArtifact>(content) {
        return Ok(artifact);
    }

    let contract: ReviewerFindingsContractArtifact = serde_json::from_str(content)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let _ = contract.verdict.as_deref();
    let _ = contract.summary.as_deref();

    Ok(ReviewArtifact {
        severity_summary: csa_session::SeveritySummary::from_findings(&contract.findings),
        findings: contract.findings,
        review_mode: None,
        schema_version: "1.0".to_string(),
        session_id: path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("unknown-reviewer")
            .to_string(),
        timestamp: chrono::Utc::now(),
    })
}

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
        let artifact = parse_reviewer_artifact(&artifact_path, &content)?;
        reviewer_artifacts.push(artifact);
    }
    Ok(reviewer_artifacts)
}

pub(super) fn write_multi_reviewer_parent_artifacts(
    project_root: &std::path::Path,
    reviewers: usize,
    outcomes: &[ReviewerOutcome],
    final_verdict: &str,
    all_reviewers_unavailable: bool,
    parent_review_meta: Option<&ReviewSessionMeta>,
) -> Result<()> {
    let Some((session_dir, session_id)) = resolve_parent_session_env() else {
        return Ok(());
    };
    let reviewer_artifacts = load_multi_reviewer_artifacts(&session_dir, reviewers)?;
    let consolidated = build_consolidated_artifact(reviewer_artifacts, &session_id);
    let parent_decision =
        parent_review_decision(&consolidated, final_verdict, all_reviewers_unavailable);
    let parent_verdict = parent_legacy_verdict(parent_decision, final_verdict);
    write_consolidated_artifact(&consolidated, &session_dir)?;
    write_parent_findings_toml(&session_dir, &consolidated)?;
    write_parent_review_verdict(
        &session_dir,
        &session_id,
        &consolidated,
        parent_decision,
        &parent_verdict,
    )?;
    if let Some(meta) = parent_review_meta {
        let mut meta = meta.clone();
        meta.decision = parent_decision.as_str().to_string();
        meta.verdict = parent_verdict.clone();
        meta.exit_code = if parent_decision.is_clean() { 0 } else { 1 };
        write_review_meta(&session_dir, &meta)
            .context("failed to write parent review_meta.json")?;
        crate::review_gate::maybe_write_gate_marker_for_clean(
            project_root,
            &meta.head_sha,
            &meta.verdict,
            outcomes.first().map(|o| o.session_id.as_str()),
            &meta.scope,
        );
    }
    write_parent_review_summary(&session_dir, outcomes, &parent_verdict)?;
    write_parent_review_details(&session_dir, outcomes)?;
    Ok(())
}

pub(super) fn write_multi_reviewer_consensus_artifacts(
    ctx: MultiReviewerConsensusArtifacts<'_>,
) -> Result<()> {
    let final_review_meta = parent_consensus_review_meta(
        ctx.head_sha,
        ctx.scope,
        ctx.final_verdict,
        ctx.review_iterations,
        ctx.diff_fingerprint.clone(),
    );
    write_multi_reviewer_parent_artifacts(
        ctx.project_root,
        ctx.reviewers,
        ctx.outcomes,
        ctx.final_verdict,
        ctx.all_reviewers_unavailable,
        final_review_meta.as_ref(),
    )?;
    if final_review_meta.is_none() {
        write_standalone_consensus_review_artifacts(&ctx)?;
    }
    Ok(())
}

pub(super) fn write_standalone_consensus_review_artifacts(
    ctx: &MultiReviewerConsensusArtifacts<'_>,
) -> Result<Option<String>> {
    let Some(target) = ctx.outcomes.first() else {
        return Ok(None);
    };
    let session_dir = csa_session::get_session_dir(ctx.project_root, &target.session_id)
        .with_context(|| format!("failed to resolve session dir for {}", target.session_id))?;
    let decision = if ctx.all_reviewers_unavailable {
        ReviewDecision::Unavailable
    } else {
        consensus_review_decision(ctx.final_verdict)
    };
    let verdict = parent_legacy_verdict(decision, ctx.final_verdict);
    let meta = ReviewSessionMeta {
        session_id: target.session_id.clone(),
        head_sha: ctx.head_sha.to_string(),
        decision: decision.as_str().to_string(),
        verdict: verdict.clone(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "consensus".to_string(),
        scope: ctx.scope.to_string(),
        exit_code: if decision.is_clean() { 0 } else { 1 },
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: ctx.review_iterations,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: ctx.diff_fingerprint.clone(),
    };
    write_review_meta(&session_dir, &meta).context("failed to write consensus review_meta.json")?;
    let verdict_artifact = ReviewVerdictArtifact::from_parts(
        target.session_id.clone(),
        decision,
        verdict,
        &[],
        Vec::new(),
    );
    write_review_verdict(&session_dir, &verdict_artifact)
        .context("failed to write consensus output/review-verdict.json")?;
    write_parent_review_summary(&session_dir, ctx.outcomes, &meta.verdict)?;
    write_parent_review_details(&session_dir, ctx.outcomes)?;
    crate::review_gate::maybe_write_gate_marker_for_clean(
        ctx.project_root,
        &meta.head_sha,
        &meta.verdict,
        Some(&target.session_id),
        &meta.scope,
    );
    Ok(Some(target.session_id.clone()))
}

pub(super) fn parent_consensus_review_meta(
    head_sha: &str,
    scope: &str,
    final_verdict: &str,
    review_iterations: u32,
    diff_fingerprint: Option<String>,
) -> Option<ReviewSessionMeta> {
    let decision = consensus_review_decision(final_verdict);
    resolve_parent_session_env().map(|(_, session_id)| ReviewSessionMeta {
        session_id,
        head_sha: head_sha.to_string(),
        decision: decision.as_str().to_string(),
        verdict: final_verdict.to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "consensus".to_string(),
        scope: scope.to_string(),
        exit_code: if decision == ReviewDecision::Pass {
            0
        } else {
            1
        },
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations,
        timestamp: chrono::Utc::now(),
        diff_fingerprint,
    })
}

fn consensus_review_decision(final_verdict: &str) -> ReviewDecision {
    match final_verdict {
        CLEAN => ReviewDecision::Pass,
        HAS_ISSUES => ReviewDecision::Fail,
        SKIP => ReviewDecision::Skip,
        UNAVAILABLE => ReviewDecision::Unavailable,
        _ => ReviewDecision::Uncertain,
    }
}

fn resolve_parent_session_env() -> Option<(PathBuf, String)> {
    if let Some(session_dir) = std::env::var_os(CSA_DAEMON_SESSION_DIR_ENV_KEY) {
        let session_id =
            std::env::var(CSA_DAEMON_SESSION_ID_ENV_KEY).unwrap_or_else(|_| "unknown".to_string());
        return Some((PathBuf::from(session_dir), session_id));
    }

    let session_dir = std::env::var_os(CSA_SESSION_DIR_ENV_KEY)?;
    let session_id =
        std::env::var(CSA_SESSION_ID_ENV_KEY).unwrap_or_else(|_| "unknown".to_string());
    Some((PathBuf::from(session_dir), session_id))
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
    decision: ReviewDecision,
    verdict_legacy: &str,
) -> Result<()> {
    let verdict = ReviewVerdictArtifact::from_parts(
        session_id.to_string(),
        decision,
        verdict_legacy.to_string(),
        &artifact.findings,
        Vec::new(),
    );
    write_review_verdict(session_dir, &verdict)
        .context("failed to write parent output/review-verdict.json")
}

fn parent_review_decision(
    artifact: &ReviewArtifact,
    final_verdict: &str,
    all_reviewers_unavailable: bool,
) -> ReviewDecision {
    if all_reviewers_unavailable {
        return ReviewDecision::Unavailable;
    }
    if artifact
        .findings
        .iter()
        .any(|finding| is_blocking_severity(&finding.severity))
    {
        return ReviewDecision::Fail;
    }
    if artifact.findings.is_empty()
        || artifact
            .findings
            .iter()
            .all(|finding| finding.severity == Severity::Low)
    {
        return ReviewDecision::Pass;
    }
    ReviewDecision::from_str(final_verdict).unwrap_or(ReviewDecision::Uncertain)
}

fn is_blocking_severity(severity: &Severity) -> bool {
    matches!(
        severity,
        Severity::Critical | Severity::High | Severity::Medium
    )
}

fn parent_legacy_verdict(decision: ReviewDecision, fallback: &str) -> String {
    match decision {
        ReviewDecision::Pass => CLEAN.to_string(),
        ReviewDecision::Fail => HAS_ISSUES.to_string(),
        ReviewDecision::Skip | ReviewDecision::Uncertain | ReviewDecision::Unavailable => {
            fallback.to_string()
        }
    }
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
#[path = "review_cmd_parent_artifacts_tests.rs"]
mod tests;
