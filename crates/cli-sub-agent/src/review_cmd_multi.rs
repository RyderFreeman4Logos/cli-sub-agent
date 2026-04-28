use anyhow::{Context, Result};
use csa_core::types::ToolName;
use tokio::task::JoinSet;
use tracing::error;

use super::output::ReviewerOutcome;
use super::result_handling::build_unavailable_reviewer_outcome;

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
