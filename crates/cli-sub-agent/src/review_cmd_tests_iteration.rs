use super::*;
use crate::review_consensus::review_iteration::count_prior_reviews_for_branch;
use crate::test_env_lock::TEST_ENV_LOCK;
use chrono::Utc;
use csa_core::types::ToolName;
use csa_session::{
    Genealogy, MetaSessionState, ReviewSessionMeta, SessionPhase, TaskContext, get_session_root,
    save_session, write_review_meta,
};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("spawn git");
    assert!(
        status.success(),
        "git command failed: git {}",
        args.join(" ")
    );
}

fn init_git_repo_with_branch(dir: &Path, branch: &str) {
    run_git(dir, &["init", "--initial-branch", branch]);
    run_git(dir, &["config", "user.name", "Test User"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    std::fs::write(dir.join("README.md"), "# test\n").expect("write README");
    run_git(dir, &["add", "README.md"]);
    run_git(dir, &["commit", "-m", "init"]);
}

fn make_review_meta(session_id: &str, decision: &str, review_iterations: u32) -> ReviewSessionMeta {
    ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: "deadbeef".to_string(),
        decision: decision.to_string(),
        verdict: decision.to_ascii_uppercase(),
        status_reason: None,
        tool: "codex".to_string(),
        scope: "uncommitted".to_string(),
        exit_code: 0,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations,
        timestamp: Utc::now(),
        diff_fingerprint: None,
    }
}

fn create_mock_review_session(
    project_root: &Path,
    session_id: &str,
    branch: Option<&str>,
    decision: &str,
    review_iterations: u32,
) {
    let session_root = get_session_root(project_root).expect("resolve session root");
    let session_dir = session_root.join("sessions").join(session_id);
    std::fs::create_dir_all(&session_dir).expect("create mock session dir");
    save_session(&MetaSessionState {
        meta_session_id: session_id.to_string(),
        description: None,
        project_path: project_root.display().to_string(),
        branch: branch.map(str::to_string),
        created_at: Utc::now(),
        last_accessed: Utc::now(),
        genealogy: Genealogy {
            parent_session_id: None,
            depth: 0,
            ..Default::default()
        },
        tools: HashMap::new(),
        context_status: Default::default(),
        total_token_usage: None,
        phase: SessionPhase::Available,
        task_context: TaskContext::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        vcs_identity: None,
        identity_version: 1,
        fork_call_timestamps: Vec::new(),
    })
    .expect("write mock session state");
    write_review_meta(
        &session_dir,
        &make_review_meta(session_id, decision, review_iterations),
    )
    .expect("write review meta");
}

#[test]
fn build_review_instruction_for_project_contains_design_preference_anchor() {
    let project_dir = tempdir().unwrap();
    let (instruction, _routing) = build_review_instruction_for_project(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
        project_dir.path(),
        resolve::ReviewProjectPromptOptions {
            project_config: None,
            prior_rounds_section: None,
        },
    );

    assert!(instruction.contains("Design preferences vs correctness bugs"));
}

#[test]
fn build_multi_reviewer_instruction_contains_design_preference_anchor() {
    let project_dir = tempdir().unwrap();
    let prompt = crate::review_consensus::build_multi_reviewer_instruction(
        "Base prompt",
        1,
        ToolName::Codex,
        project_dir.path(),
        None,
    );

    assert!(prompt.contains("Design preferences vs correctness bugs"));
}

#[test]
fn count_prior_reviews_zero_omits_iteration_block() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("test env lock");
    let project_dir = tempdir().unwrap();
    init_git_repo_with_branch(project_dir.path(), "feat/iter-zero");

    assert_eq!(
        count_prior_reviews_for_branch(project_dir.path(), Some("feat/iter-zero")),
        0
    );

    let (instruction, _routing) = build_review_instruction_for_project(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
        project_dir.path(),
        resolve::ReviewProjectPromptOptions {
            project_config: None,
            prior_rounds_section: None,
        },
    );

    assert!(!instruction.contains("## Review iteration context"));
}

#[test]
fn count_prior_reviews_one_injects_iteration_two() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("test env lock");
    let project_dir = tempdir().unwrap();
    init_git_repo_with_branch(project_dir.path(), "feat/iter-one");
    create_mock_review_session(
        project_dir.path(),
        "01K7ER7A0E0000000000000001",
        Some("feat/iter-one"),
        "fail",
        1,
    );

    assert_eq!(
        count_prior_reviews_for_branch(project_dir.path(), Some("feat/iter-one")),
        1
    );

    let (instruction, _routing) = build_review_instruction_for_project(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
        project_dir.path(),
        resolve::ReviewProjectPromptOptions {
            project_config: None,
            prior_rounds_section: None,
        },
    );

    assert!(instruction.contains("This is review iteration 2 on branch 'feat/iter-one'."));
    assert!(instruction.contains("Prior review count on this branch: 1."));
}

#[test]
fn count_prior_reviews_three_adds_multi_round_escalation() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("test env lock");
    let project_dir = tempdir().unwrap();
    init_git_repo_with_branch(project_dir.path(), "feat/iter-three");
    create_mock_review_session(
        project_dir.path(),
        "01K7ER7A0E0000000000000011",
        Some("feat/iter-three"),
        "fail",
        1,
    );
    create_mock_review_session(
        project_dir.path(),
        "01K7ER7A0E0000000000000012",
        Some("feat/iter-three"),
        "fail",
        2,
    );
    create_mock_review_session(
        project_dir.path(),
        "01K7ER7A0E0000000000000013",
        Some("feat/iter-three"),
        "pass",
        3,
    );

    assert_eq!(
        count_prior_reviews_for_branch(project_dir.path(), Some("feat/iter-three")),
        3
    );

    let prompt = crate::review_consensus::build_multi_reviewer_instruction(
        "Base prompt",
        2,
        ToolName::Codex,
        project_dir.path(),
        None,
    );

    assert!(prompt.contains("Prior review count on this branch: 3."));
    assert!(prompt.contains("Multiple prior rounds have fired on this branch."));
    assert!(prompt.contains("Oscillating DESIGN choices"));
    assert!(prompt.contains("Still broken from last round is still broken."));
}

#[test]
fn multi_round_escalation_keeps_persistent_correctness_bugs_blocking() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("test env lock");
    let project_dir = tempdir().unwrap();
    init_git_repo_with_branch(project_dir.path(), "feat/iter-persistent-bug");
    create_mock_review_session(
        project_dir.path(),
        "01K7ER7A0E0000000000000041",
        Some("feat/iter-persistent-bug"),
        "fail",
        1,
    );
    create_mock_review_session(
        project_dir.path(),
        "01K7ER7A0E0000000000000042",
        Some("feat/iter-persistent-bug"),
        "fail",
        2,
    );
    create_mock_review_session(
        project_dir.path(),
        "01K7ER7A0E0000000000000043",
        Some("feat/iter-persistent-bug"),
        "pass",
        3,
    );

    let prompt = crate::review_consensus::build_multi_reviewer_instruction(
        "Base prompt",
        2,
        ToolName::Codex,
        project_dir.path(),
        None,
    );

    assert!(prompt.contains("persistent correctness bugs remain blocking"));
}

#[test]
fn build_multi_reviewer_instruction_uses_explicit_project_root_outside_cwd() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("test env lock");
    let project_dir = tempdir().unwrap();
    let unrelated_dir = tempdir().unwrap();
    init_git_repo_with_branch(project_dir.path(), "feat/iter-explicit-root");
    create_mock_review_session(
        project_dir.path(),
        "01K7ER7A0E0000000000000091",
        Some("feat/iter-explicit-root"),
        "fail",
        1,
    );

    let current_dir = std::env::current_dir().expect("current dir");
    assert_ne!(current_dir, project_dir.path());
    assert_ne!(unrelated_dir.path(), project_dir.path());

    let prompt = crate::review_consensus::build_multi_reviewer_instruction(
        "Base prompt",
        2,
        ToolName::Codex,
        project_dir.path(),
        None,
    );

    assert!(prompt.contains("This is review iteration 2 on branch 'feat/iter-explicit-root'."));
}

#[test]
fn count_prior_reviews_does_not_pull_reviews_from_other_branches() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("test env lock");
    let project_dir = tempdir().unwrap();
    init_git_repo_with_branch(project_dir.path(), "feat/iter-current");
    create_mock_review_session(
        project_dir.path(),
        "01K7ER7A0E0000000000000021",
        Some("feat/iter-other"),
        "fail",
        1,
    );
    create_mock_review_session(
        project_dir.path(),
        "01K7ER7A0E0000000000000022",
        Some("feat/iter-other"),
        "pass",
        2,
    );

    assert_eq!(
        count_prior_reviews_for_branch(project_dir.path(), Some("feat/iter-current")),
        0
    );
}

#[test]
fn count_prior_reviews_branch_unknown_returns_safe_zero() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("test env lock");
    let project_dir = tempdir().unwrap();
    init_git_repo_with_branch(project_dir.path(), "feat/iter-unknown");
    create_mock_review_session(
        project_dir.path(),
        "01K7ER7A0E0000000000000031",
        Some("feat/iter-a"),
        "fail",
        1,
    );
    create_mock_review_session(
        project_dir.path(),
        "01K7ER7A0E0000000000000032",
        Some("feat/iter-b"),
        "pass",
        2,
    );

    // Branch-unknown must yield zero to avoid cross-branch contamination; mirror
    // review_context.rs:187 behavior.
    assert_eq!(count_prior_reviews_for_branch(project_dir.path(), None), 0);
}

#[test]
fn count_prior_reviews_uses_canonical_max_after_more_than_ten_prior_reviews() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("test env lock");
    let project_dir = tempdir().unwrap();
    init_git_repo_with_branch(project_dir.path(), "feat/iter-many");

    for iteration in 1..=12 {
        create_mock_review_session(
            project_dir.path(),
            &format!("01K7ER7A0E{:016}", iteration),
            Some("feat/iter-many"),
            "fail",
            iteration,
        );
    }

    assert_eq!(
        count_prior_reviews_for_branch(project_dir.path(), Some("feat/iter-many")),
        12
    );
}

#[path = "review_cmd_tests_checklist.rs"]
mod review_cmd_tests_checklist;
#[path = "review_cmd_tests_prior_rounds.rs"]
mod review_cmd_tests_prior_rounds;
