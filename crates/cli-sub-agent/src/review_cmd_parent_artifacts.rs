use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use csa_core::env::CSA_SESSION_DIR_ENV_KEY;
use csa_core::types::ReviewDecision;
use csa_session::review_artifact::{Finding, ReviewArtifact, Severity, SeveritySummary};
use csa_session::state::{ReviewSessionMeta, write_review_meta};
use csa_session::{
    FindingsFile, ReviewFinding, ReviewFindingFileRange, ReviewVerdictArtifact,
    write_findings_toml, write_review_verdict,
};
use serde::Deserialize;

use crate::review_cmd::artifact_parse::parse_review_artifact_fields_lossy;
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

pub(super) fn clear_multi_reviewer_artifact_dirs(reviewers: usize) -> Result<()> {
    let Some((session_dir, _session_id)) = resolve_parent_session_env() else {
        return Ok(());
    };

    for reviewer_index in 1..=reviewers {
        let reviewer_dir = session_dir.join(format!("reviewer-{reviewer_index}"));
        match fs::remove_dir_all(&reviewer_dir) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to clear {}", reviewer_dir.display()));
            }
        }
    }
    Ok(())
}

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

    if let Ok(fields) = parse_review_artifact_fields_lossy(content) {
        return Ok(ReviewArtifact {
            severity_summary: fields.severity_summary,
            findings: fields.findings,
            review_mode: None,
            schema_version: "1.0".to_string(),
            session_id: path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .unwrap_or("unknown-reviewer")
                .to_string(),
            timestamp: chrono::Utc::now(),
        });
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
    project_root: &Path,
    output_dir: &Path,
    reviewers: usize,
    outcomes: &[ReviewerOutcome],
) -> Result<(Vec<ReviewArtifact>, BTreeSet<usize>)> {
    let mut reviewer_artifacts = Vec::new();
    let mut persisted_indices = BTreeSet::new();
    for reviewer_index in 1..=reviewers {
        let mut artifact_paths = vec![
            output_dir
                .join(format!("reviewer-{reviewer_index}"))
                .join("review-findings.json"),
        ];
        if let Some(outcome) = outcomes
            .iter()
            .find(|outcome| outcome.reviewer_index + 1 == reviewer_index)
            && let Ok(session_dir) = csa_session::get_session_dir(project_root, &outcome.session_id)
        {
            artifact_paths.push(
                session_dir
                    .join(format!("reviewer-{reviewer_index}"))
                    .join("review-findings.json"),
            );
        }

        for artifact_path in artifact_paths {
            if !artifact_path.exists() {
                continue;
            }

            let content = fs::read_to_string(&artifact_path)
                .with_context(|| format!("failed to read {}", artifact_path.display()))?;
            let artifact = parse_reviewer_artifact(&artifact_path, &content)?;
            reviewer_artifacts.push(artifact);
            persisted_indices.insert(reviewer_index);
            break;
        }
    }
    Ok((reviewer_artifacts, persisted_indices))
}

/// Whether every reviewer that voted `HAS_ISSUES` persisted a structured findings
/// artifact. When false, at least one dissenting reviewer's findings never reached
/// disk (e.g. quota/auth failure forced a non-zero exit before structured output was
/// written), so an empty consolidated artifact does NOT prove "no issues exist" and the
/// parent gate must fail-closed rather than promote to PASS. Crucially this is per-reviewer:
/// one OTHER reviewer persisting an empty artifact must not mask an unpersisted dissenter
/// (#1659).
fn dissenting_findings_persisted(
    outcomes: &[ReviewerOutcome],
    persisted_indices: &BTreeSet<usize>,
) -> bool {
    outcomes
        .iter()
        .filter(|outcome| outcome.verdict == HAS_ISSUES)
        .all(|outcome| persisted_indices.contains(&(outcome.reviewer_index + 1)))
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
    let (reviewer_artifacts, persisted_indices) =
        load_multi_reviewer_artifacts(project_root, &session_dir, reviewers, outcomes)?;
    let dissent_findings_persisted = dissenting_findings_persisted(outcomes, &persisted_indices);
    let consolidated = build_consolidated_artifact(reviewer_artifacts, &session_id);
    let parent_decision = parent_review_decision(
        &consolidated,
        final_verdict,
        all_reviewers_unavailable,
        dissent_findings_persisted,
    );
    let parent_verdict = parent_legacy_verdict(parent_decision, final_verdict);
    let parent_artifact = parent_artifact_for_decision(&consolidated, parent_decision);
    write_consolidated_artifact(&parent_artifact, &session_dir)?;
    write_parent_findings_toml(&session_dir, &parent_artifact)?;
    write_parent_review_verdict(
        &session_dir,
        &session_id,
        &consolidated.findings,
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
    let Some((target, session_dir)) = resolve_standalone_consensus_carrier(ctx)? else {
        return Ok(None);
    };
    let (reviewer_artifacts, persisted_indices) =
        load_multi_reviewer_artifacts(ctx.project_root, &session_dir, ctx.reviewers, ctx.outcomes)?;
    let dissent_findings_persisted =
        dissenting_findings_persisted(ctx.outcomes, &persisted_indices);
    let consolidated = build_consolidated_artifact(reviewer_artifacts, &target.session_id);
    let decision = parent_review_decision(
        &consolidated,
        ctx.final_verdict,
        ctx.all_reviewers_unavailable,
        dissent_findings_persisted,
    );
    let verdict = parent_legacy_verdict(decision, ctx.final_verdict);
    let artifact = parent_artifact_for_decision(&consolidated, decision);
    write_consolidated_artifact(&artifact, &session_dir)?;
    write_parent_findings_toml(&session_dir, &artifact)?;
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
        &consolidated.findings,
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

fn resolve_standalone_consensus_carrier<'a>(
    ctx: &'a MultiReviewerConsensusArtifacts<'_>,
) -> Result<Option<(&'a ReviewerOutcome, PathBuf)>> {
    for outcome in ctx.outcomes {
        let session_dir = csa_session::get_session_dir(ctx.project_root, &outcome.session_id)
            .with_context(|| format!("failed to resolve session dir for {}", outcome.session_id))?;
        if session_dir.is_dir()
            && csa_session::load_session(ctx.project_root, &outcome.session_id).is_ok()
        {
            return Ok(Some((outcome, session_dir)));
        }
    }
    Ok(None)
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
    severity_count_findings: &[Finding],
    decision: ReviewDecision,
    verdict_legacy: &str,
) -> Result<()> {
    let verdict = ReviewVerdictArtifact::from_parts(
        session_id.to_string(),
        decision,
        verdict_legacy.to_string(),
        severity_count_findings,
        Vec::new(),
    );
    write_review_verdict(session_dir, &verdict)
        .context("failed to write parent output/review-verdict.json")
}

fn parent_artifact_for_decision(
    artifact: &ReviewArtifact,
    parent_decision: ReviewDecision,
) -> ReviewArtifact {
    if parent_decision != ReviewDecision::Pass {
        return artifact.clone();
    }

    let findings: Vec<Finding> = artifact
        .findings
        .iter()
        .filter(|finding| !is_blocking_severity(&finding.severity))
        .cloned()
        .collect();
    ReviewArtifact {
        severity_summary: SeveritySummary::from_findings(&findings),
        findings,
        review_mode: artifact.review_mode.clone(),
        schema_version: artifact.schema_version.clone(),
        session_id: artifact.session_id.clone(),
        timestamp: artifact.timestamp,
    }
}

fn parent_review_decision(
    artifact: &ReviewArtifact,
    final_verdict: &str,
    all_reviewers_unavailable: bool,
    dissent_findings_persisted: bool,
) -> ReviewDecision {
    if all_reviewers_unavailable {
        return ReviewDecision::Unavailable;
    }
    let consensus_decision =
        ReviewDecision::from_str(final_verdict).unwrap_or(ReviewDecision::Uncertain);
    if artifact
        .findings
        .iter()
        .any(|finding| is_blocking_severity(&finding.severity))
    {
        return ReviewDecision::Fail;
    }
    // #1659 false-PASS guard: a non-clean consensus (a reviewer voted HAS_ISSUES) whose
    // consolidated artifact is empty is only trustworthy as PASS when EVERY dissenting
    // reviewer persisted its structured findings. If any HAS_ISSUES voter never persisted
    // (e.g. quota/auth failure forced a non-zero exit before structured output was written),
    // an empty/synthetic findings.toml does NOT mean "no findings exist" -- and one OTHER
    // reviewer's empty artifact must not mask that unpersisted dissent (#1659 round-2, codex).
    // Fail-closed on the consensus verdict rather than promoting to PASS. When every dissenter
    // DID persist (even an empty artifact), the explicit "no findings" is trusted as a genuine
    // PASS, preserving the #1045/#1217 zero-findings-pass behavior.
    if !dissent_findings_persisted
        && artifact.findings.is_empty()
        && consensus_decision == ReviewDecision::Fail
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
    if consensus_decision == ReviewDecision::Fail {
        return ReviewDecision::Fail;
    }
    if consensus_decision == ReviewDecision::Pass {
        return ReviewDecision::Pass;
    }
    consensus_decision
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
