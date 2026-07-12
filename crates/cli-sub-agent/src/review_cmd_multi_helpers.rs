use super::*;

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

pub(super) fn consensus_response_from_outcome(outcome: &ReviewerOutcome) -> AgentResponse {
    AgentResponse {
        agent: format!(
            "reviewer-{}:{}",
            outcome.reviewer_index + 1,
            outcome.tool.as_str()
        ),
        content: outcome.verdict.to_string(),
        weight: 1.0,
        timed_out: !outcome.produced_usable_verdict(),
    }
}

pub(super) fn consensus_outcomes_for_final_verdict(
    outcomes: &[ReviewerOutcome],
) -> Vec<&ReviewerOutcome> {
    let has_usable_verdict = outcomes
        .iter()
        .any(ReviewerOutcome::produced_usable_verdict);
    outcomes
        .iter()
        .filter(|outcome| !(has_usable_verdict && outcome_has_permanent_quota_unavailable(outcome)))
        .collect()
}

pub(super) fn permanent_quota_unavailable_reviewer_indices(
    outcomes: &[ReviewerOutcome],
) -> Vec<usize> {
    let has_usable_verdict = outcomes
        .iter()
        .any(ReviewerOutcome::produced_usable_verdict);
    if !has_usable_verdict {
        return Vec::new();
    }
    outcomes
        .iter()
        .filter(|outcome| outcome_has_permanent_quota_unavailable(outcome))
        .map(|outcome| outcome.reviewer_index)
        .collect()
}

pub(super) fn outcome_has_permanent_quota_unavailable(outcome: &ReviewerOutcome) -> bool {
    permanent_quota_unavailable_fallback_attempt(outcome).is_some()
}

pub(super) fn permanent_quota_unavailable_fallback_chain(
    outcomes: &[ReviewerOutcome],
) -> Vec<FallbackAttempt> {
    let has_usable_verdict = outcomes
        .iter()
        .any(ReviewerOutcome::produced_usable_verdict);
    if !has_usable_verdict {
        return Vec::new();
    }
    outcomes
        .iter()
        .filter_map(permanent_quota_unavailable_fallback_attempt)
        .collect()
}

pub(super) fn permanent_quota_unavailable_fallback_attempt(
    outcome: &ReviewerOutcome,
) -> Option<FallbackAttempt> {
    let diagnostic = outcome.diagnostic.as_deref()?;
    let kind = FailoverSkipKind::classify(diagnostic);
    (outcome.verdict == UNAVAILABLE && kind.is_quota()).then(|| FallbackAttempt {
        tool: outcome.tool.as_str().to_string(),
        model_spec: None,
        skip_reason: kind.category().to_string(),
        quota_exhausted: true,
        timestamp: chrono::Utc::now(),
    })
}

pub(super) fn persist_excluded_reviewer_routing(
    project_root: &Path,
    review_routing: &ReviewRoutingMetadata,
    outcomes: &[ReviewerOutcome],
    fallback_chain: &[FallbackAttempt],
) {
    if fallback_chain.is_empty() {
        return;
    }
    for outcome in outcomes
        .iter()
        .filter(|outcome| outcome.produced_usable_verdict())
    {
        persist_review_routing_artifact_with_fallback_chain(
            project_root,
            &outcome.session_id,
            review_routing,
            fallback_chain,
        );
    }
}

pub(super) fn multi_reviewer_exit_code(final_verdict: &str) -> i32 {
    if final_verdict == CLEAN { 0 } else { 1 }
}
