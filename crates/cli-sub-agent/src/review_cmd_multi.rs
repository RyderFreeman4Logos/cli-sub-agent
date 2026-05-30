use anyhow::{Context, Result};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::consensus::AgentResponse;
use csa_core::types::ToolName;
use tokio::task::JoinSet;
use tracing::{error, warn};

use crate::cli::ReviewArgs;
use crate::pipeline::resolve_effective_initial_response_timeout_for_tool;
use crate::review_consensus::{
    CLEAN, UNAVAILABLE, agreement_level, build_multi_reviewer_instruction,
    consensus_strategy_label, consensus_verdict, parse_consensus_strategy, resolve_consensus,
};
use crate::review_routing::ReviewRoutingMetadata;

use super::bug_class_pipeline::{
    maybe_extract_recurring_bug_class_skills, resolve_review_iterations,
};
use super::execute::{compute_diff_fingerprint, execute_review_with_tier_filter};
use super::output::ReviewerOutcome;
use super::output::{
    GEMINI_AUTH_PROMPT_STATUS_REASON, persist_review_meta, persist_review_verdict,
    print_reviewer_outcomes,
};
use super::prior_rounds::explicit_review_tool;
use super::result_handling::{
    build_reviewer_outcome, build_unavailable_reviewer_outcome, reviewer_unavailable_error_reason,
};
use super::reviewers::resolve_multi_reviewer_pool;
use csa_session::state::ReviewSessionMeta;
use std::path::Path;

pub(super) struct MultiReviewerReviewContext<'a> {
    pub args: &'a ReviewArgs,
    pub reviewers: usize,
    pub tool: ToolName,
    pub prompt: &'a str,
    pub scope: &'a str,
    pub project_root: &'a Path,
    pub config: &'a Option<ProjectConfig>,
    pub global_config: &'a GlobalConfig,
    pub pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    pub review_routing: ReviewRoutingMetadata,
    pub review_model: Option<String>,
    pub resolved_model_spec: Option<String>,
    pub resolved_tier_name: Option<String>,
    pub review_thinking: Option<String>,
    pub stream_mode: csa_process::StreamMode,
    pub idle_timeout_seconds: u64,
    pub readonly_project_root: bool,
    pub prior_rounds_section: Option<&'a str>,
}

pub(super) async fn run_multi_reviewer_review(ctx: MultiReviewerReviewContext<'_>) -> Result<i32> {
    if ctx.args.fix {
        anyhow::bail!("--fix is not supported when --reviewers > 1");
    }
    if ctx.args.session.is_some() {
        anyhow::bail!("--session is only supported when --reviewers=1");
    }

    let consensus_strategy = parse_consensus_strategy(&ctx.args.consensus)?;
    let reviewer_pool = resolve_multi_reviewer_pool(
        ctx.reviewers,
        explicit_review_tool(ctx.args),
        ctx.tool,
        ctx.resolved_tier_name.as_deref(),
        ctx.config.as_ref(),
        ctx.global_config,
    )?;
    let reviewer_tools = reviewer_pool.reviewer_tools;
    let reviewer_tool_plan = reviewer_tools.clone();
    let tier_reviewer_specs = reviewer_pool.tier_reviewer_specs;
    warn_if_fast_mode_has_no_codex_reviewer(
        ctx.args.fast_but_more_cost,
        &reviewer_tool_plan,
        &tier_reviewer_specs,
    );
    super::parent_artifacts::clear_multi_reviewer_artifact_dirs(ctx.reviewers)?;

    let mut join_set = JoinSet::new();
    for (reviewer_index, reviewer_tool) in reviewer_tools.into_iter().enumerate() {
        let reviewer_prompt = build_multi_reviewer_instruction(
            ctx.prompt,
            reviewer_index + 1,
            reviewer_tool,
            ctx.project_root,
            ctx.prior_rounds_section,
        );
        let reviewer_model = ctx.review_model.clone();
        let reviewer_project_root = ctx.project_root.to_path_buf();
        let reviewer_config = ctx.config.as_ref().cloned();
        let reviewer_global = ctx.global_config.clone();
        let reviewer_pre_session_hook = ctx.pre_session_hook.clone();
        let reviewer_description = format!(
            "review[{}]: {}",
            reviewer_index + 1,
            crate::run_helpers::truncate_prompt(ctx.scope, 80)
        );
        let reviewer_routing = ctx.review_routing.clone();

        let reviewer_force_override = ctx.args.force_override_user_config;
        let reviewer_force_ignore_tier = ctx.args.force_ignore_tier_setting;
        let reviewer_no_failover = ctx.args.no_failover;
        let reviewer_fast_but_more_cost = ctx.args.fast_but_more_cost;
        let reviewer_no_fs_sandbox = ctx.args.no_fs_sandbox;
        let reviewer_extra_writable = ctx.args.extra_writable.clone();
        let reviewer_extra_readable = ctx.args.extra_readable.clone();
        // Keep every reviewer on the resolved tier when possible by binding
        // each tool to its tier model spec. Fall back to the primary spec only
        // when we only have a single tier-resolved reviewer tool.
        let reviewer_model_spec = tier_reviewer_specs
            .iter()
            .find(|resolution| resolution.tool == reviewer_tool)
            .map(|resolution| resolution.model_spec.clone())
            .or_else(|| {
                if reviewer_tool == ctx.tool {
                    ctx.resolved_model_spec.clone()
                } else {
                    None
                }
            });
        let reviewer_tier_name = ctx.resolved_tier_name.clone();
        let reviewer_thinking = ctx.review_thinking.clone();
        let reviewer_initial_response_timeout_seconds =
            resolve_effective_initial_response_timeout_for_tool(
                reviewer_config.as_ref(),
                ctx.args.initial_response_timeout,
                ctx.args.idle_timeout,
                ctx.args.timeout,
                reviewer_tool.as_str(),
            );
        let stream_mode = ctx.stream_mode;
        let idle_timeout_seconds = ctx.idle_timeout_seconds;
        let readonly_project_root = ctx.readonly_project_root;
        join_set.spawn(async move {
            let session_result = match execute_review_with_tier_filter(
                reviewer_tool,
                reviewer_prompt,
                None,
                reviewer_model,
                reviewer_model_spec,
                reviewer_tier_name,
                false,
                None,
                reviewer_thinking,
                reviewer_description,
                &reviewer_project_root,
                reviewer_config.as_ref(),
                &reviewer_global,
                reviewer_pre_session_hook,
                reviewer_routing,
                stream_mode,
                idle_timeout_seconds,
                reviewer_initial_response_timeout_seconds,
                reviewer_force_override,
                reviewer_force_ignore_tier,
                reviewer_no_failover,
                reviewer_fast_but_more_cost,
                false,
                reviewer_no_fs_sandbox,
                readonly_project_root,
                &reviewer_extra_writable,
                &reviewer_extra_readable,
            )
            .await
            {
                Ok(session_result) => session_result,
                Err(err) => {
                    if let Some(reason) = reviewer_unavailable_error_reason(&err, reviewer_tool) {
                        warn!(
                            reviewer = reviewer_index + 1,
                            tool = %reviewer_tool,
                            reason = %reason,
                            "Reviewer unavailable; continuing multi-reviewer consensus"
                        );
                        return Ok(build_unavailable_reviewer_outcome(
                            reviewer_index,
                            reviewer_tool,
                            reason,
                        ));
                    }
                    return Err(err);
                }
            };
            build_reviewer_outcome(reviewer_index, reviewer_tool, &session_result)
        });
    }

    let outcomes =
        collect_reviewer_outcomes(&mut join_set, &reviewer_tool_plan, ctx.args.timeout).await?;

    let review_iterations = outcomes
        .first()
        .map(|outcome| resolve_review_iterations(ctx.project_root, &outcome.session_id))
        .unwrap_or(1);
    let head_sha = csa_session::detect_git_head(ctx.project_root).unwrap_or_default();
    let diff_fingerprint = compute_diff_fingerprint(ctx.project_root, ctx.scope);
    persist_multi_review_sidecars(
        ctx.project_root,
        ctx.scope,
        &outcomes,
        &head_sha,
        review_iterations,
        diff_fingerprint.clone(),
    );

    let responses: Vec<AgentResponse> = outcomes
        .iter()
        .map(consensus_response_from_outcome)
        .collect();

    let consensus_result = resolve_consensus(consensus_strategy, &responses);
    let all_reviewers_unavailable = !outcomes.is_empty()
        && outcomes
            .iter()
            .all(|outcome| outcome.verdict == UNAVAILABLE);
    let final_verdict = if all_reviewers_unavailable {
        crate::review_consensus::UNAVAILABLE
    } else {
        consensus_verdict(&consensus_result)
    };
    let agreement = agreement_level(&consensus_result);
    let consensus_artifacts = super::parent_artifacts::MultiReviewerConsensusArtifacts {
        project_root: ctx.project_root,
        reviewers: ctx.reviewers,
        outcomes: &outcomes,
        final_verdict,
        all_reviewers_unavailable,
        head_sha: &head_sha,
        scope: ctx.scope,
        review_iterations,
        diff_fingerprint: diff_fingerprint.clone(),
    };
    if let Err(err) =
        super::parent_artifacts::write_multi_reviewer_consensus_artifacts(consensus_artifacts)
    {
        warn!(
            error = %err,
            "Failed to write multi-reviewer consensus artifacts (continuing)"
        );
    }

    print_reviewer_outcomes(&outcomes);

    println!(
        "===== Consensus =====\nstrategy: {}\nconsensus_reached: {}\nagreement_level: {:.0}%\nfinal_decision: {final_verdict}\nindividual_verdicts:",
        consensus_strategy_label(consensus_result.strategy_used),
        consensus_result.consensus_reached,
        agreement * 100.0,
    );
    for outcome in &outcomes {
        println!(
            "- reviewer {} ({}) => {}",
            outcome.reviewer_index + 1,
            outcome.tool,
            outcome.verdict
        );
    }

    let review_session_ids = outcomes
        .iter()
        .map(|outcome| outcome.session_id.clone())
        .collect::<Vec<_>>();
    maybe_extract_recurring_bug_class_skills(ctx.project_root, &review_session_ids);
    Ok(if final_verdict == CLEAN { 0 } else { 1 })
}

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

fn consensus_response_from_outcome(outcome: &ReviewerOutcome) -> AgentResponse {
    AgentResponse {
        agent: format!(
            "reviewer-{}:{}",
            outcome.reviewer_index + 1,
            outcome.tool.as_str()
        ),
        content: outcome.verdict.to_string(),
        weight: 1.0,
        timed_out: outcome.verdict == UNAVAILABLE,
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
        let effective_meta = super::output::fail_closed_review_meta(project_root, &review_meta);
        persist_review_meta(project_root, &effective_meta);
        super::findings_toml::persist_review_findings_toml(project_root, &effective_meta);
        persist_review_verdict(project_root, &effective_meta, &[], Vec::new());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_consensus::HAS_ISSUES;
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
        ReviewerOutcome {
            reviewer_index,
            tool: ToolName::Codex,
            session_id: format!("01TESTREVIEWER{reviewer_index:012}"),
            output: verdict.to_string(),
            exit_code: if verdict == CLEAN { 0 } else { 1 },
            verdict,
            diagnostic: None,
        }
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
}
