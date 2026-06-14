use std::{fs, path::Path};

use csa_core::consensus::ConsensusResult;
use csa_core::types::ReviewDecision;
use csa_session::{ReviewArtifact, SeveritySummary};
use tracing::warn;

use crate::review_consensus::{HAS_ISSUES, UNAVAILABLE, consensus_verdict};

use super::output::ReviewerOutcome;

pub(super) fn apply_repo_write_audit_findings_to_multi_outcomes(
    project_root: &Path,
    outcomes: &mut [ReviewerOutcome],
) -> bool {
    let mut blocked = false;
    for outcome in outcomes {
        if super::dirty_tree::repo_write_audit_findings(project_root, &outcome.session_id)
            .is_empty()
        {
            continue;
        }
        blocked = true;
        outcome.verdict = HAS_ISSUES;
        outcome.exit_code =
            crate::verdict_exit_code::exit_code_from_review_decision(ReviewDecision::Fail);
        if !outcome.output.ends_with('\n') {
            outcome.output.push('\n');
        }
        outcome.output.push_str(
            "[csa-review] Read-only reviewer mutated repo-tracked file(s); \
             see CSA-REVIEW-WORKTREE-MUTATION finding.\n",
        );
    }
    blocked
}

pub(super) fn final_verdict_for_multi_review(
    all_reviewers_unavailable: bool,
    repo_write_audit_blocked: bool,
    consensus_result: &ConsensusResult,
) -> &'static str {
    if all_reviewers_unavailable {
        UNAVAILABLE
    } else if repo_write_audit_blocked {
        HAS_ISSUES
    } else {
        consensus_verdict(consensus_result)
    }
}

pub(super) fn persist_multi_reviewer_repo_write_audit_artifact(
    project_root: &Path,
    parent_session_dir: Option<&Path>,
    outcome: &ReviewerOutcome,
    findings: &[csa_session::Finding],
    review_mode: Option<&str>,
) {
    if findings.is_empty() {
        return;
    }
    let artifact = ReviewArtifact {
        findings: findings.to_vec(),
        severity_summary: SeveritySummary::from_findings(findings),
        review_mode: review_mode.map(str::to_string),
        schema_version: "1.0".to_string(),
        session_id: outcome.session_id.clone(),
        timestamp: chrono::Utc::now(),
    };
    let reviewer_dir_name = format!("reviewer-{}", outcome.reviewer_index + 1);
    if let Ok(session_dir) = csa_session::get_session_dir(project_root, &outcome.session_id) {
        write_repo_write_audit_artifact(
            &session_dir
                .join(&reviewer_dir_name)
                .join("review-findings.json"),
            &artifact,
        );
    }
    if let Some(parent_session_dir) = parent_session_dir {
        merge_repo_write_audit_artifact(
            &parent_session_dir
                .join(reviewer_dir_name)
                .join("review-findings.json"),
            &artifact,
        );
    }
}

fn write_repo_write_audit_artifact(path: &Path, artifact: &ReviewArtifact) {
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        warn!(
            path = %parent.display(),
            error = %error,
            "Failed to create multi-reviewer repo-write audit artifact directory"
        );
        return;
    }
    let payload = match serde_json::to_vec_pretty(&artifact) {
        Ok(payload) => payload,
        Err(error) => {
            warn!(
                session_id = %artifact.session_id,
                error = %error,
                "Failed to serialize multi-reviewer repo-write audit artifact"
            );
            return;
        }
    };
    if let Err(error) = fs::write(path, payload) {
        warn!(
            session_id = %artifact.session_id,
            path = %path.display(),
            error = %error,
            "Failed to write multi-reviewer repo-write audit artifact"
        );
    }
}

fn merge_repo_write_audit_artifact(path: &Path, audit_artifact: &ReviewArtifact) {
    let artifact = fs::read_to_string(path)
        .ok()
        .and_then(|content| match super::parent_artifacts::parse_reviewer_artifact(path, &content) {
            Ok(mut existing) => {
                for finding in &audit_artifact.findings {
                    existing
                        .findings
                        .retain(|existing| existing.fid != finding.fid);
                    existing.findings.push(finding.clone());
                }
                existing.severity_summary = SeveritySummary::from_findings(&existing.findings);
                if existing.review_mode.is_none() {
                    existing.review_mode = audit_artifact.review_mode.clone();
                }
                existing.timestamp = chrono::Utc::now();
                Some(existing)
            }
            Err(error) => {
                warn!(
                    path = %path.display(),
                    error = %error,
                    "Failed to parse existing parent reviewer artifact; replacing with repo-write audit artifact"
                );
                None
            }
        })
        .unwrap_or_else(|| audit_artifact.clone());
    write_repo_write_audit_artifact(path, &artifact);
}
