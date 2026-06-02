use super::*;
use crate::review_consensus::HAS_ISSUES;
use csa_config::ProjectProfile;
use csa_core::consensus::ConsensusStrategy;
use proptest::prelude::*;

#[derive(Clone, Copy, Debug)]
enum ReviewerState {
    Pass,
    Fail,
    Unavailable,
}

impl ReviewerState {
    fn verdict(self) -> &'static str {
        match self {
            Self::Pass => CLEAN,
            Self::Fail => HAS_ISSUES,
            Self::Unavailable => UNAVAILABLE,
        }
    }
}

fn outcome(reviewer_index: usize, verdict: &'static str) -> ReviewerOutcome {
    outcome_with_tool(reviewer_index, ToolName::Codex, verdict, None)
}

fn outcome_with_tool(
    reviewer_index: usize,
    tool: ToolName,
    verdict: &'static str,
    diagnostic: Option<&str>,
) -> ReviewerOutcome {
    ReviewerOutcome {
        reviewer_index,
        tool,
        session_id: format!("01TESTREVIEWER{reviewer_index:012}"),
        output: verdict.to_string(),
        exit_code: if verdict == CLEAN { 0 } else { 1 },
        verdict,
        diagnostic: diagnostic.map(str::to_string),
    }
}

#[test]
fn parent_startup_env_for_multi_review_uses_daemon_session_context() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let session_id = "01PARENTSESSION000000000000";

    let startup_env = parent_startup_env_for_multi_review(
        true,
        Some(session_id),
        &crate::startup_env::StartupSubtreeEnv::default(),
        temp.path(),
    )
    .expect("startup env should accept daemon session context");

    let expected_session_dir =
        csa_session::get_session_dir(temp.path(), session_id).expect("session dir should resolve");
    let expected_session_dir = expected_session_dir.to_string_lossy().to_string();
    assert_eq!(startup_env.session_id(), Some(session_id));
    assert_eq!(
        startup_env.session_dir(),
        Some(expected_session_dir.as_str())
    );
}

#[test]
fn unavailable_outcomes_do_not_vote_against_clean_consensus() {
    let outcomes = [outcome(0, UNAVAILABLE), outcome(1, CLEAN)];
    let responses: Vec<AgentResponse> = outcomes
        .iter()
        .map(consensus_response_from_outcome)
        .collect();
    let consensus = resolve_consensus(ConsensusStrategy::Majority, &responses);

    assert_eq!(consensus.decision.as_deref(), Some(CLEAN));
    assert_eq!(consensus_verdict(&consensus), CLEAN);
}

#[test]
fn permanent_quota_unavailable_reviewer_is_excluded_when_peer_passes() {
    let outcomes = [
        outcome_with_tool(
            0,
            ToolName::GeminiCli,
            UNAVAILABLE,
            Some("gemini-cli OAuth quota exhausted; monthly cap reached"),
        ),
        outcome_with_tool(1, ToolName::Codex, CLEAN, None),
    ];

    let consensus_outcomes = consensus_outcomes_for_final_verdict(&outcomes);
    assert_eq!(
        consensus_outcomes
            .iter()
            .map(|outcome| outcome.reviewer_index)
            .collect::<Vec<_>>(),
        vec![1]
    );

    let responses: Vec<AgentResponse> = consensus_outcomes
        .iter()
        .map(|outcome| consensus_response_from_outcome(outcome))
        .collect();
    let consensus = resolve_consensus(ConsensusStrategy::Majority, &responses);

    assert_eq!(consensus.decision.as_deref(), Some(CLEAN));
    assert_eq!(consensus_verdict(&consensus), CLEAN);
    assert_eq!(multi_reviewer_exit_code(consensus_verdict(&consensus)), 0);

    let fallback_chain = permanent_quota_unavailable_fallback_chain(&outcomes);
    assert_eq!(fallback_chain.len(), 1);
    assert_eq!(fallback_chain[0].tool, "gemini-cli");
    assert_eq!(fallback_chain[0].skip_reason, "oauth-quota");
    assert!(fallback_chain[0].quota_exhausted);
}

#[test]
fn all_permanent_quota_unavailable_reviewers_still_fail_unavailable() {
    let outcomes = [
        outcome_with_tool(
            0,
            ToolName::GeminiCli,
            UNAVAILABLE,
            Some("gemini-cli OAuth quota exhausted; monthly cap reached"),
        ),
        outcome_with_tool(
            1,
            ToolName::ClaudeCode,
            UNAVAILABLE,
            Some("OAuth quota exhausted; billing monthly spending cap reached"),
        ),
    ];

    let consensus_outcomes = consensus_outcomes_for_final_verdict(&outcomes);
    assert_eq!(consensus_outcomes.len(), outcomes.len());
    assert!(permanent_quota_unavailable_reviewer_indices(&outcomes).is_empty());
    assert!(
        outcomes
            .iter()
            .all(|outcome| outcome.verdict == UNAVAILABLE)
    );
    assert!(permanent_quota_unavailable_fallback_chain(&outcomes).is_empty());
    assert_eq!(multi_reviewer_exit_code(UNAVAILABLE), 1);
}

#[test]
fn review_routing_artifact_records_excluded_quota_reviewer_skip_reason() {
    let outcomes = [
        outcome_with_tool(
            0,
            ToolName::GeminiCli,
            UNAVAILABLE,
            Some("gemini-cli OAuth quota exhausted; monthly cap reached"),
        ),
        outcome_with_tool(1, ToolName::Codex, CLEAN, None),
    ];
    let fallback_chain = permanent_quota_unavailable_fallback_chain(&outcomes);
    let routing = ReviewRoutingMetadata {
        project_profile: ProjectProfile::Unknown,
        detection_method: "auto",
    };

    let artifact = crate::review_routing::render_review_routing_artifact(&routing, &fallback_chain);
    let parsed: serde_json::Value = serde_json::from_str(&artifact).expect("review-routing json");
    let chain = parsed["fallback_chain"]
        .as_array()
        .expect("fallback_chain array");

    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0]["tool"], "gemini-cli");
    assert_eq!(chain[0]["skip_reason"], "oauth-quota");
    assert_eq!(chain[0]["quota_exhausted"], true);
}

#[test]
fn unavailable_outcomes_do_not_hide_has_issues_consensus() {
    let outcomes = [outcome(0, UNAVAILABLE), outcome(1, HAS_ISSUES)];
    let responses: Vec<AgentResponse> = outcomes
        .iter()
        .map(consensus_response_from_outcome)
        .collect();
    let consensus = resolve_consensus(ConsensusStrategy::Majority, &responses);

    assert_eq!(consensus.decision.as_deref(), Some(HAS_ISSUES));
    assert_eq!(consensus_verdict(&consensus), HAS_ISSUES);
}

#[tokio::test]
async fn collect_reviewer_outcomes_waits_after_unavailable_reviewer() {
    let mut join_set = JoinSet::new();
    join_set.spawn(async {
        Ok(build_unavailable_reviewer_outcome(
            0,
            ToolName::GeminiCli,
            "gemini-cli tool failure: reason: QUOTA_EXHAUSTED",
        ))
    });
    join_set.spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        Ok(outcome(1, CLEAN))
    });

    let outcomes =
        collect_reviewer_outcomes(&mut join_set, &[ToolName::GeminiCli, ToolName::Codex], None)
            .await
            .expect("collect outcomes");

    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes[0].verdict, UNAVAILABLE);
    assert_eq!(outcomes[1].verdict, CLEAN);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn reviewer_outcome_consensus_mapping_filters_unavailable_before_vote(
        states in prop::collection::vec(reviewer_state_strategy(), 2..=4),
    ) {
        let outcomes: Vec<ReviewerOutcome> = states
            .iter()
            .enumerate()
            .map(|(idx, state)| outcome(idx, state.verdict()))
            .collect();
        let responses: Vec<AgentResponse> = outcomes
            .iter()
            .map(consensus_response_from_outcome)
            .collect();
        let active_responses: Vec<AgentResponse> = responses
            .iter()
            .filter(|response| !response.timed_out)
            .cloned()
            .collect();

        prop_assert_eq!(
            responses.iter().filter(|response| response.timed_out).count(),
            states.iter().filter(|state| matches!(state, ReviewerState::Unavailable)).count()
        );

        let consensus_with_unavailable =
            resolve_consensus(ConsensusStrategy::Majority, &responses);
        let consensus_without_unavailable =
            resolve_consensus(ConsensusStrategy::Majority, &active_responses);

        prop_assert_eq!(
            consensus_with_unavailable.decision.as_deref(),
            consensus_without_unavailable.decision.as_deref()
        );
        prop_assert_eq!(
            consensus_verdict(&consensus_with_unavailable),
            consensus_verdict(&consensus_without_unavailable)
        );
    }
}

fn reviewer_state_strategy() -> impl Strategy<Value = ReviewerState> {
    prop_oneof![
        Just(ReviewerState::Pass),
        Just(ReviewerState::Fail),
        Just(ReviewerState::Unavailable),
    ]
}
