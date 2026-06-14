use super::*;
use crate::review_consensus::{HAS_ISSUES, consensus_verdict};
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use csa_config::ProjectProfile;
use csa_core::consensus::ConsensusStrategy;
use csa_core::env::{CSA_SESSION_DIR_ENV_KEY, CSA_SESSION_ID_ENV_KEY};
use csa_core::types::ReviewDecision;
use csa_session::{FindingsFile, ReviewArtifact, ReviewVerdictArtifact, Severity, SeveritySummary};
use proptest::prelude::*;
use std::{collections::HashMap, path::Path};

use super::super::multi_repo_write_audit::{
    apply_repo_write_audit_findings_to_multi_outcomes, final_verdict_for_multi_review,
};

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
    outcome_with_session_id(
        reviewer_index,
        tool,
        format!("01TESTREVIEWER{reviewer_index:012}"),
        verdict,
        diagnostic,
    )
}

fn outcome_with_session_id(
    reviewer_index: usize,
    tool: ToolName,
    session_id: String,
    verdict: &'static str,
    diagnostic: Option<&str>,
) -> ReviewerOutcome {
    ReviewerOutcome {
        reviewer_index,
        tool,
        session_id,
        output: verdict.to_string(),
        exit_code: if verdict == CLEAN { 0 } else { 1 },
        verdict,
        diagnostic: diagnostic.map(str::to_string),
    }
}

fn create_review_session(project_root: &Path, prompt: &str) -> (String, std::path::PathBuf) {
    let session =
        csa_session::create_session_fresh(project_root, Some(prompt), None, Some("codex"))
            .expect("create review session");
    csa_session::save_session(&session).expect("save review session");
    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id)
        .expect("resolve review session dir");
    std::fs::create_dir_all(session_dir.join("output")).expect("create review output dir");
    (session.meta_session_id, session_dir)
}

fn save_result_with_repo_write_audit(project_root: &Path, session_id: &str) {
    let mut repo_write_audit = toml::map::Map::new();
    repo_write_audit.insert(
        "modified".to_string(),
        toml::Value::Array(vec![toml::Value::String("weave.lock".to_string())]),
    );
    let mut artifacts = toml::map::Map::new();
    artifacts.insert(
        "repo_write_audit".to_string(),
        toml::Value::Table(repo_write_audit),
    );

    let now = chrono::Utc::now();
    let result = csa_session::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "Clean textual verdict".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(1),
        manager_fields: csa_session::SessionManagerFields {
            artifacts: Some(toml::Value::Table(artifacts)),
            ..Default::default()
        },
        ..Default::default()
    };
    csa_session::save_result(project_root, session_id, &result).expect("save review result");
}

fn startup_env_for_parent_session(
    session_dir: &Path,
    session_id: &str,
) -> crate::startup_env::StartupSubtreeEnv {
    crate::startup_env::StartupSubtreeEnv::from_values(HashMap::from([
        (CSA_SESSION_DIR_ENV_KEY, session_dir.display().to_string()),
        (CSA_SESSION_ID_ENV_KEY, session_id.to_string()),
    ]))
}

#[test]
fn repo_write_audit_blocks_clean_multi_reviewer_majority_and_parent_verdict() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let project = tempfile::tempdir().expect("tempdir should be created");
    let _state_home = ScopedEnvVarRestore::set("XDG_STATE_HOME", project.path().join("state"));

    let (dirty_session_id, dirty_session_dir) =
        create_review_session(project.path(), "dirty reviewer");
    save_result_with_repo_write_audit(project.path(), &dirty_session_id);
    let (clean_session_id_1, _) = create_review_session(project.path(), "clean reviewer 1");
    let (clean_session_id_2, _) = create_review_session(project.path(), "clean reviewer 2");

    let mut outcomes = vec![
        outcome_with_session_id(0, ToolName::Codex, dirty_session_id.clone(), CLEAN, None),
        outcome_with_session_id(1, ToolName::Codex, clean_session_id_1, CLEAN, None),
        outcome_with_session_id(2, ToolName::Codex, clean_session_id_2, CLEAN, None),
    ];

    let blocked = apply_repo_write_audit_findings_to_multi_outcomes(project.path(), &mut outcomes);

    assert!(blocked);
    assert_eq!(outcomes[0].verdict, HAS_ISSUES);
    assert_eq!(outcomes[0].exit_code, 1);
    assert!(outcomes[0].output.contains("CSA-REVIEW-WORKTREE-MUTATION"));

    let responses: Vec<AgentResponse> = consensus_outcomes_for_final_verdict(&outcomes)
        .iter()
        .map(|outcome| consensus_response_from_outcome(outcome))
        .collect();
    let consensus = resolve_consensus(ConsensusStrategy::Majority, &responses);
    assert_eq!(
        consensus_verdict(&consensus),
        CLEAN,
        "the regression requires an explicit audit override because raw majority would pass"
    );
    let final_verdict = final_verdict_for_multi_review(false, blocked, &consensus);
    assert_eq!(final_verdict, HAS_ISSUES);

    let parent_dir = project.path().join("parent-session");
    std::fs::create_dir_all(parent_dir.join("reviewer-1")).expect("create parent reviewer dir");
    let parent_session_id = "01PARENTSESSION000000000000";
    let preexisting_parent_artifact = ReviewArtifact {
        findings: Vec::new(),
        severity_summary: SeveritySummary::default(),
        review_mode: Some("standard".to_string()),
        schema_version: "1.0".to_string(),
        session_id: dirty_session_id.clone(),
        timestamp: chrono::Utc::now(),
    };
    std::fs::write(
        parent_dir.join("reviewer-1").join("review-findings.json"),
        serde_json::to_vec_pretty(&preexisting_parent_artifact)
            .expect("preexisting parent artifact should serialize"),
    )
    .expect("preexisting parent reviewer artifact should be written");

    persist_multi_review_sidecars(
        project.path(),
        Some(&parent_dir),
        "range:main...HEAD",
        &outcomes,
        "HEADSHA",
        ReviewRunMeta {
            review_iterations: 1,
            diff_fingerprint: None,
            review_mode: Some("standard"),
        },
        super::super::diff_size::ReviewDiffReport {
            diff_size: None,
            large_diff_warning: None,
        },
    );

    let child_findings: FindingsFile = toml::from_str(
        &std::fs::read_to_string(dirty_session_dir.join("output").join("findings.toml"))
            .expect("child findings.toml should exist"),
    )
    .expect("child findings.toml should parse");
    let child_finding = child_findings
        .findings
        .iter()
        .find(|finding| finding.id == "CSA-REVIEW-WORKTREE-MUTATION")
        .expect("child audit finding should be present");
    assert_eq!(child_finding.severity, Severity::High);
    assert_eq!(child_finding.file_ranges[0].path, "weave.lock");

    let child_verdict: ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(dirty_session_dir.join("output").join("review-verdict.json"))
            .expect("child review-verdict.json should exist"),
    )
    .expect("child verdict should parse");
    assert_eq!(child_verdict.decision, ReviewDecision::Fail);
    assert_eq!(child_verdict.verdict_legacy, HAS_ISSUES);
    assert_eq!(child_verdict.severity_counts.get(&Severity::High), Some(&1));

    let reviewer_artifact: ReviewArtifact = serde_json::from_str(
        &std::fs::read_to_string(
            dirty_session_dir
                .join("reviewer-1")
                .join("review-findings.json"),
        )
        .expect("fallback reviewer artifact should exist"),
    )
    .expect("fallback reviewer artifact should parse");
    assert_eq!(reviewer_artifact.severity_summary.high, 1);
    assert_eq!(
        reviewer_artifact.findings[0].fid,
        "CSA-REVIEW-WORKTREE-MUTATION"
    );

    let parent_reviewer_artifact: ReviewArtifact = serde_json::from_str(
        &std::fs::read_to_string(parent_dir.join("reviewer-1").join("review-findings.json"))
            .expect("parent reviewer artifact should exist"),
    )
    .expect("parent reviewer artifact should parse");
    assert_eq!(parent_reviewer_artifact.severity_summary.high, 1);
    assert_eq!(
        parent_reviewer_artifact.findings[0].fid,
        "CSA-REVIEW-WORKTREE-MUTATION"
    );

    super::super::parent_artifacts::write_multi_reviewer_consensus_artifacts(
        super::super::parent_artifacts::MultiReviewerConsensusArtifacts {
            project_root: project.path(),
            reviewers: outcomes.len(),
            outcomes: &outcomes,
            final_verdict,
            all_reviewers_unavailable: false,
            head_sha: "HEADSHA",
            scope: "range:main...HEAD",
            run_review_mode: Some("standard"),
            review_iterations: 1,
            diff_fingerprint: None,
            diff_size: None,
            large_diff_warning: None,
        },
        &startup_env_for_parent_session(&parent_dir, parent_session_id),
    )
    .expect("parent consensus artifacts should be written");

    let parent_findings: FindingsFile = toml::from_str(
        &std::fs::read_to_string(parent_dir.join("output").join("findings.toml"))
            .expect("parent findings.toml should exist"),
    )
    .expect("parent findings.toml should parse");
    assert_eq!(
        parent_findings.findings[0].id,
        "CSA-REVIEW-WORKTREE-MUTATION"
    );

    let parent_consolidated: ReviewArtifact = serde_json::from_str(
        &std::fs::read_to_string(
            parent_dir.join(crate::bug_class::CONSOLIDATED_REVIEW_ARTIFACT_FILE),
        )
        .expect("parent consolidated review artifact should exist"),
    )
    .expect("parent consolidated review artifact should parse");
    assert_eq!(parent_consolidated.severity_summary.high, 1);
    assert_eq!(
        parent_consolidated.findings[0].fid,
        "CSA-REVIEW-WORKTREE-MUTATION"
    );

    let parent_verdict: ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(parent_dir.join("output").join("review-verdict.json"))
            .expect("parent review-verdict.json should exist"),
    )
    .expect("parent verdict should parse");
    assert_eq!(parent_verdict.decision, ReviewDecision::Fail);
    assert_eq!(parent_verdict.verdict_legacy, HAS_ISSUES);
    assert_eq!(
        parent_verdict.severity_counts.get(&Severity::High),
        Some(&1)
    );
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
