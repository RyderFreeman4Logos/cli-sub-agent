use super::*;
use crate::cli::{Cli, Commands, validate_review_args};
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use chrono::{TimeZone, Utc};
use clap::Parser;
use csa_core::vcs::{VcsIdentity, VcsKind};
use std::process::Command;
use tempfile::TempDir;

fn write_review_session(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
    scope: &str,
    decision: ReviewDecision,
    legacy_verdict: &str,
) -> String {
    write_review_session_with_parent(
        project_root,
        branch,
        head_sha,
        scope,
        decision,
        legacy_verdict,
        None,
    )
}

fn write_review_session_with_parent(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
    scope: &str,
    decision: ReviewDecision,
    legacy_verdict: &str,
    parent_id: Option<&str>,
) -> String {
    write_review_session_with_description(
        project_root,
        branch,
        head_sha,
        scope,
        decision,
        legacy_verdict,
        parent_id,
        "review: test",
    )
}

fn write_review_session_with_description(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
    scope: &str,
    decision: ReviewDecision,
    legacy_verdict: &str,
    parent_id: Option<&str>,
    description: &str,
) -> String {
    let mut session =
        csa_session::create_session_fresh(project_root, Some(description), parent_id, None)
            .expect("create session");
    session.branch = Some(branch.to_string());
    session.git_head_at_creation = Some(head_sha.to_string());
    session.vcs_identity = Some(VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some(head_sha.to_string()),
        change_id: None,
        short_id: Some(short_sha(head_sha).to_string()),
        ref_name: Some(branch.to_string()),
        op_id: None,
    });
    csa_session::save_session(&session).expect("save session state");

    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id).unwrap();
    let meta = ReviewSessionMeta {
        session_id: session.meta_session_id.clone(),
        head_sha: head_sha.to_string(),
        decision: decision.as_str().to_string(),
        verdict: legacy_verdict.to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "codex".to_string(),
        scope: scope.to_string(),
        exit_code: if decision == ReviewDecision::Pass {
            0
        } else {
            1
        },
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: Utc::now(),
        diff_fingerprint: None,
    };
    csa_session::state::write_review_meta(&session_dir, &meta).expect("write review meta");
    csa_session::write_review_verdict(
        &session_dir,
        &ReviewVerdictArtifact::from_parts(
            session.meta_session_id.clone(),
            decision,
            legacy_verdict,
            &[],
            Vec::new(),
        ),
    )
    .expect("write review verdict");

    session.meta_session_id
}

fn set_task_type(project_root: &Path, session_id: &str, task_type: &str) {
    let mut session = csa_session::load_session(project_root, session_id).expect("load session");
    session.task_context.task_type = Some(task_type.to_string());
    csa_session::save_session(&session).expect("save session");
}

fn set_review_diff_fingerprint(
    project_root: &Path,
    session_id: &str,
    diff_fingerprint: Option<&str>,
) {
    let session_dir = csa_session::get_session_dir(project_root, session_id).unwrap();
    let mut meta = read_review_meta(&session_dir)
        .unwrap()
        .expect("review meta should exist");
    meta.diff_fingerprint = diff_fingerprint.map(str::to_string);
    csa_session::state::write_review_meta(&session_dir, &meta).expect("write review meta");
}

fn set_review_timestamp(project_root: &Path, session_id: &str, timestamp: i64) {
    let session_dir = csa_session::get_session_dir(project_root, session_id).unwrap();
    let mut meta = read_review_meta(&session_dir)
        .unwrap()
        .expect("review meta should exist");
    meta.timestamp = Utc.timestamp_opt(timestamp, 0).single().unwrap();
    csa_session::state::write_review_meta(&session_dir, &meta).expect("write review meta");
}

fn parse_review_args(argv: &[&str]) -> ReviewArgs {
    let cli = Cli::try_parse_from(argv).expect("review CLI args should parse");
    match cli.command {
        Commands::Review(args) => {
            validate_review_args(&args).expect("review CLI args should validate");
            args
        }
        _ => panic!("expected review subcommand"),
    }
}

fn run_git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git command should execute");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn setup_git_repo() -> TempDir {
    let temp = TempDir::new().expect("create tempdir");
    run_git(temp.path(), &["init"]);
    run_git(temp.path(), &["config", "user.email", "test@example.com"]);
    run_git(temp.path(), &["config", "user.name", "Test User"]);
    std::fs::write(temp.path().join("tracked.txt"), "baseline\n").expect("write tracked file");
    run_git(temp.path(), &["add", "tracked.txt"]);
    run_git(temp.path(), &["commit", "-m", "initial"]);
    temp
}

#[test]
fn check_verdict_finds_pass_for_current_branch_head_and_full_diff() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let session_id = write_review_session(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );

    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    )
    .unwrap()
    .expect("expected matching verdict");
    assert_eq!(found.session_id, session_id);
}

#[test]
fn check_verdict_rejects_stale_diff_fingerprint() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let session_id = write_review_session(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );
    set_review_diff_fingerprint(&project, &session_id, Some("sha256:old"));

    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        Some("sha256:new"),
    )
    .unwrap();
    assert!(found.is_none(), "stale diff review must not satisfy gate");

    set_review_diff_fingerprint(&project, &session_id, Some("sha256:new"));
    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        Some("sha256:new"),
    )
    .unwrap()
    .expect("matching diff fingerprint should satisfy gate");
    assert_eq!(found.session_id, session_id);
}

#[test]
fn check_verdict_rejects_child_pass_when_parent_consensus_fails() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let parent_session_id = write_review_session(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Fail,
        "HAS_ISSUES",
    );
    write_review_session_with_parent(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
        Some(&parent_session_id),
    );
    set_review_timestamp(&project, &parent_session_id, 200);
    let child_session_id = csa_session::list_sessions_from_root_readonly(
        &csa_session::get_session_root(&project).unwrap(),
    )
    .unwrap()
    .into_iter()
    .map(|session| session.meta_session_id)
    .find(|session_id| session_id != &parent_session_id)
    .unwrap();
    set_review_timestamp(&project, &child_session_id, 100);

    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    )
    .unwrap();
    assert!(found.is_none(), "child reviewer pass must not satisfy gate");
}

#[test]
fn check_verdict_rejects_when_latest_matching_verdict_fails() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let older_pass = write_review_session(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );
    let newer_fail = write_review_session(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Fail,
        "HAS_ISSUES",
    );
    set_review_timestamp(&project, &older_pass, 100);
    set_review_timestamp(&project, &newer_fail, 200);

    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    )
    .unwrap();
    assert!(
        found.is_none(),
        "newer failed review must supersede older clean review"
    );
}

#[test]
fn check_verdict_accepts_when_latest_matching_verdict_passes() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let older_fail = write_review_session(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Fail,
        "HAS_ISSUES",
    );
    let newer_pass = write_review_session(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );
    set_review_timestamp(&project, &older_fail, 100);
    set_review_timestamp(&project, &newer_pass, 200);

    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    )
    .unwrap()
    .expect("newer clean review should satisfy gate");
    assert_eq!(found.session_id, newer_pass);
}

#[test]
fn check_verdict_accepts_reviewer_sub_session_pass_without_parent_consensus() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let sub_session_id = write_review_session_with_description(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
        None,
        "review[1]: range:main...HEAD",
    );
    set_task_type(&project, &sub_session_id, "reviewer_sub_session");

    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    )
    .unwrap()
    .expect("reviewer sub-session pass should satisfy gate when it is the latest match");
    assert_eq!(found.session_id, sub_session_id);
}

#[test]
fn check_verdict_accepts_consensus_parent_named_like_reviewer() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let session_id = write_review_session_with_description(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
        None,
        "review[2]: range:main...HEAD",
    );
    set_task_type(&project, &session_id, "review");

    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    )
    .unwrap()
    .expect("consensus parent session should satisfy gate");
    assert_eq!(found.session_id, session_id);
}

#[test]
fn check_verdict_rejects_non_pass_artifact_even_when_meta_is_pass() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    for (decision, legacy_verdict) in [
        (ReviewDecision::Fail, "HAS_ISSUES"),
        (ReviewDecision::Uncertain, "UNCERTAIN"),
        (ReviewDecision::Skip, "SKIP"),
        (ReviewDecision::Unavailable, "UNAVAILABLE"),
    ] {
        let temp = TempDir::new().unwrap();
        let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        let session_id = write_review_session(
            &project,
            "feature",
            "abcdef1234567890",
            REQUIRED_FULL_DIFF_SCOPE,
            ReviewDecision::Pass,
            "CLEAN",
        );
        let session_dir = csa_session::get_session_dir(&project, &session_id).unwrap();
        csa_session::write_review_verdict(
            &session_dir,
            &ReviewVerdictArtifact::from_parts(
                session_id,
                decision,
                legacy_verdict,
                &[],
                Vec::new(),
            ),
        )
        .expect("write non-pass review verdict");

        let found = check_review_verdict_for_target(
            &project,
            "feature",
            "abcdef1234567890",
            REQUIRED_FULL_DIFF_SCOPE,
            None,
        )
        .unwrap();
        assert!(
            found.is_none(),
            "expected no match for non-pass artifact decision {decision}"
        );
    }
}

#[test]
fn check_verdict_does_not_recover_or_rewrite_corrupt_session_state() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let corrupt_session =
        csa_session::create_session_fresh(&project, Some("corrupt session"), None, None)
            .expect("create corrupt session");
    let corrupt_dir =
        csa_session::get_session_dir(&project, &corrupt_session.meta_session_id).unwrap();
    let corrupt_state_path = corrupt_dir.join("state.toml");
    let corrupt_state = "this is not valid toml";
    std::fs::write(&corrupt_state_path, corrupt_state).expect("corrupt state");

    let session_id = write_review_session(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );

    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    )
    .unwrap()
    .expect("expected matching verdict despite unrelated corrupt session");
    assert_eq!(found.session_id, session_id);
    assert_eq!(
        std::fs::read_to_string(&corrupt_state_path).expect("read corrupt state"),
        corrupt_state
    );
    assert!(!corrupt_dir.join("state.toml.corrupt").exists());
}

#[test]
fn check_verdict_rejects_commit_review_even_when_clean() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    write_review_session(
        &project,
        "feature",
        "abcdef1234567890",
        "commit:abcdef1234567890",
        ReviewDecision::Pass,
        "CLEAN",
    );

    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    )
    .unwrap();
    assert!(found.is_none());
}

#[test]
fn check_verdict_rejects_stale_head() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    write_review_session(
        &project,
        "feature",
        "old1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );

    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "new1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    )
    .unwrap();
    assert!(found.is_none());
}

#[test]
fn check_verdict_uses_requested_range_scope() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = setup_git_repo();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    run_git(temp.path(), &["checkout", "-b", "feature"]);
    let branch = run_git(temp.path(), &["branch", "--show-current"]);
    let head_sha = csa_session::detect_git_head(temp.path()).unwrap();
    write_review_session(
        temp.path(),
        &branch,
        &head_sha,
        "range:release...HEAD",
        ReviewDecision::Pass,
        "CLEAN",
    );

    let args = parse_review_args(&[
        "csa",
        "review",
        "--check-verdict",
        "--range",
        "release...HEAD",
    ]);

    let exit = handle_check_verdict(temp.path(), &args).unwrap();
    assert_eq!(exit, 0);
}
