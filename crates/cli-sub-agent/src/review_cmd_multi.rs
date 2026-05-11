use anyhow::{Context, Result};
use csa_core::types::ToolName;
use tokio::task::JoinSet;
use tracing::error;

use super::output::ReviewerOutcome;
use super::output::{
    GEMINI_AUTH_PROMPT_STATUS_REASON, persist_review_meta, persist_review_verdict,
};
use super::result_handling::build_unavailable_reviewer_outcome;
use csa_session::state::ReviewSessionMeta;
use std::path::Path;

pub(super) fn warn_if_fast_mode_has_no_codex_reviewer(
    fast_but_more_cost: bool,
    reviewer_tools: &[ToolName],
    tier_reviewer_specs: &[crate::run_helpers::TierToolResolution],
) {
    if fast_but_more_cost
        && !reviewer_tools.contains(&ToolName::Codex)
        && !tier_reviewer_specs
            .iter()
            .any(|resolution| resolution.tool == ToolName::Codex)
    {
        eprintln!(
            "warning: --fast-but-more-cost only affects codex; no codex review attempt is in the resolved candidate set."
        );
    }
}

pub(super) async fn collect_reviewer_outcomes(
    join_set: &mut JoinSet<Result<ReviewerOutcome>>,
    reviewer_tools: &[ToolName],
    timeout_secs: Option<u64>,
) -> Result<Vec<ReviewerOutcome>> {
    let mut outcomes: Vec<ReviewerOutcome> = Vec::with_capacity(reviewer_tools.len());
    let reviewer_timeout = timeout_secs.map(std::time::Duration::from_secs);
    let deadline = reviewer_timeout.map(|timeout| tokio::time::Instant::now() + timeout);
    while outcomes.len() < reviewer_tools.len() {
        let joined = if let (Some(timeout), Some(dl)) = (reviewer_timeout, deadline) {
            match tokio::time::timeout_at(dl, join_set.join_next()).await {
                Ok(joined) => joined,
                Err(_) => {
                    error!(
                        timeout_secs = timeout.as_secs(),
                        completed_reviewers = outcomes.len(),
                        total_reviewers = reviewer_tools.len(),
                        "Reviewer timed out; marking incomplete reviewers UNAVAILABLE"
                    );
                    join_set.abort_all();
                    synthesize_unavailable_outcomes(&mut outcomes, reviewer_tools, timeout);
                    break;
                }
            }
        } else {
            join_set.join_next().await
        };

        let Some(joined) = joined else {
            break;
        };
        let outcome = joined.context("reviewer task join failure")??;
        outcomes.push(outcome);
    }
    outcomes.sort_by_key(|o| o.reviewer_index);
    Ok(outcomes)
}

pub(super) fn persist_multi_review_sidecars(
    project_root: &Path,
    scope: &str,
    outcomes: &[ReviewerOutcome],
    head_sha: &str,
    review_iterations: u32,
    diff_fingerprint: Option<String>,
) {
    let review_meta_timestamp = chrono::Utc::now();

    for outcome in outcomes {
        let review_meta = ReviewSessionMeta {
            session_id: outcome.session_id.clone(),
            head_sha: head_sha.to_string(),
            decision: super::flow::review_decision_from_verdict(outcome.verdict)
                .as_str()
                .to_string(),
            verdict: outcome.verdict.to_string(),
            status_reason: (outcome.verdict == "UNCERTAIN"
                && outcome
                    .diagnostic
                    .as_deref()
                    .is_some_and(|d| d.contains("OAuth browser prompt")))
            .then(|| GEMINI_AUTH_PROMPT_STATUS_REASON.to_string()),
            routed_to: None,
            primary_failure: None,
            failure_reason: None,
            tool: outcome.tool.as_str().to_string(),
            scope: scope.to_string(),
            exit_code: outcome.exit_code,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations,
            timestamp: review_meta_timestamp,
            diff_fingerprint: diff_fingerprint.clone(),
        };
        persist_review_meta(project_root, &review_meta);
        super::findings_toml::persist_review_findings_toml(project_root, &review_meta);
        persist_review_verdict(project_root, &review_meta, &[], Vec::new());
    }
}

fn synthesize_unavailable_outcomes(
    outcomes: &mut Vec<ReviewerOutcome>,
    reviewer_tools: &[ToolName],
    timeout: std::time::Duration,
) {
    for (reviewer_index, reviewer_tool) in reviewer_tools.iter().enumerate() {
        if outcomes.iter().any(|o| o.reviewer_index == reviewer_index) {
            continue;
        }
        outcomes.push(build_unavailable_reviewer_outcome(
            reviewer_index,
            *reviewer_tool,
            format!("reviewer timed out after {}s", timeout.as_secs()),
        ));
    }
}
