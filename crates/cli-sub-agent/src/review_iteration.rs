use std::path::Path;

use super::review_iteration_resolver::try_max_review_iterations_for_branch;

const REVIEW_ITERATION_HEADER: &str = "## Review iteration context";
const MULTI_ROUND_ESCALATION: &str = "Multiple prior rounds have fired on this branch. Oscillating DESIGN choices (preserve-vs-overwrite semantics, where-to-persist artifacts, API surface shapes, which lifecycle stage triggers side-effects) across rounds indicate design residuals, not bugs — prefer PASS on those.\n\nHOWEVER: persistent correctness bugs remain blocking even if raised in a prior round. If a prior round flagged a crash path, data loss, contract violation, or security issue, and the current diff has NOT fixed it, FAIL on it — repetition does not convert a real bug into a design residual. Still broken from last round is still broken.\n\nOnly treat a finding as a design residual when prior rounds oscillated between contradictory fixes (A vs not-A) on a point of taste, AND the current choice has a coherent rationale.";

pub(crate) fn count_prior_reviews_for_branch(project_root: &Path, branch: Option<&str>) -> usize {
    let current_session_id = std::env::var("CSA_SESSION_ID").ok();
    match branch {
        Some(branch) => try_max_review_iterations_for_branch(
            project_root,
            branch,
            current_session_id.as_deref(),
        )
        .map(|iterations| iterations as usize)
        .unwrap_or(0),
        None => 0,
    }
}

pub(crate) fn render_review_iteration_context(project_root: &Path, branch: &str) -> Option<String> {
    let prior_count = count_prior_reviews_for_branch(project_root, Some(branch));
    if prior_count == 0 {
        return None;
    }

    let mut rendered = format!(
        "{REVIEW_ITERATION_HEADER}\n\nThis is review iteration {} on branch '{branch}'. Prior review count on this branch: {prior_count}.\n",
        prior_count + 1
    );
    if prior_count >= 3 {
        rendered.push('\n');
        rendered.push_str(MULTI_ROUND_ESCALATION);
        rendered.push('\n');
    }
    Some(rendered)
}
