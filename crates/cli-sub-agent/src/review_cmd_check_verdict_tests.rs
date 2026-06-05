use super::*;
use crate::cli::{Cli, Commands, validate_review_args};
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use chrono::{TimeZone, Utc};
use clap::Parser;
use csa_core::types::ReviewDecision;
use csa_core::vcs::{VcsIdentity, VcsKind};
use csa_session::Finding;
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

#[allow(clippy::too_many_arguments)]
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
        review_mode: None,
        fix_convergence: None,
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

fn write_review_session_with_findings(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
    scope: &str,
    decision: ReviewDecision,
    legacy_verdict: &str,
    findings: &[Finding],
) -> String {
    let session_id = write_review_session(
        project_root,
        branch,
        head_sha,
        scope,
        decision,
        legacy_verdict,
    );
    let session_dir = csa_session::get_session_dir(project_root, &session_id).unwrap();
    csa_session::write_review_verdict(
        &session_dir,
        &ReviewVerdictArtifact::from_parts(
            session_id.clone(),
            decision,
            legacy_verdict,
            findings,
            Vec::new(),
        ),
    )
    .expect("write review verdict with findings");
    session_id
}

fn finding_with_severity(severity: Severity, fid: &str) -> Finding {
    Finding {
        severity,
        fid: fid.to_string(),
        file: "src/lib.rs".to_string(),
        line: Some(7),
        rule_id: format!("rule.review.{fid}"),
        summary: "review finding".to_string(),
        engine: "reviewer".to_string(),
    }
}

fn set_task_type(project_root: &Path, session_id: &str, task_type: &str) {
    let mut session = csa_session::load_session(project_root, session_id).expect("load session");
    session.task_context.task_type = Some(task_type.to_string());
    csa_session::save_session(&session).expect("save session");
}

fn set_session_branch(project_root: &Path, session_id: &str, branch: &str) {
    let mut session = csa_session::load_session(project_root, session_id).expect("load session");
    session.branch = Some(branch.to_string());
    if let Some(identity) = session.vcs_identity.as_mut() {
        identity.ref_name = Some(branch.to_string());
    }
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
        None,
    )
    .unwrap()
    .expect("expected matching verdict");
    assert_eq!(found.session_id, session_id);
}

#[test]
fn issue_1696_check_verdict_pass_message_surfaces_nonblocking_counts() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = setup_git_repo();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    run_git(temp.path(), &["checkout", "-b", "feature"]);
    let branch = run_git(temp.path(), &["branch", "--show-current"]);
    let head_sha = csa_session::detect_git_head(temp.path()).unwrap();
    let findings = vec![
        finding_with_severity(Severity::Medium, "FID-MEDIUM"),
        finding_with_severity(Severity::Low, "FID-LOW-1"),
        finding_with_severity(Severity::Low, "FID-LOW-2"),
        finding_with_severity(Severity::Low, "FID-LOW-3"),
    ];
    let session_id = write_review_session_with_findings(
        temp.path(),
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
        &findings,
    );

    let found = check_review_verdict_for_target(
        temp.path(),
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .unwrap()
    .expect("expected matching verdict");
    assert_eq!(found.session_id, session_id);
    assert_eq!(found.severity_counts.get(&Severity::Medium), Some(&1));
    assert_eq!(found.severity_counts.get(&Severity::Low), Some(&3));

    let message = format_review_verdict_pass_message(&found, &branch);
    assert!(
        message.contains("PASS/CLEAN"),
        "pass verdict must remain visible: {message}"
    );
    assert!(
        message.contains("non-blocking findings: 1 medium, 3 low"),
        "non-blocking findings must be surfaced: {message}"
    );

    let args = parse_review_args(&["csa", "review", "--check-verdict"]);
    let exit = handle_check_verdict(temp.path(), &args).unwrap();
    assert_eq!(exit, 0);
}

#[test]
fn check_verdict_reads_review_marker_before_session_scan() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let branch = "feature";
    let head_sha = "abcdef1234567890";
    let session_id = write_review_session(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );
    crate::review_gate::write_review_gate_marker(
        &project,
        branch,
        head_sha,
        &session_id,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    );

    set_session_branch(&project, &session_id, "other-branch");

    let found = check_review_verdict_for_target(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .unwrap()
    .expect("marker should identify the matching verdict session");
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
        None,
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
        None,
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
        (ReviewDecision::Uncertain, "UNAVAILABLE"),
        (ReviewDecision::Uncertain, "CLEAN"),
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
fn issue_1716_check_verdict_rejects_failed_reviewer_meta_even_when_artifact_says_pass() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = setup_git_repo();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    run_git(temp.path(), &["checkout", "-b", "feature"]);
    let branch = run_git(temp.path(), &["branch", "--show-current"]);
    let head_sha = csa_session::detect_git_head(temp.path()).unwrap();

    let session_id = write_review_session(
        temp.path(),
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );
    let session_dir = csa_session::get_session_dir(temp.path(), &session_id).unwrap();
    let mut meta = read_review_meta(&session_dir)
        .unwrap()
        .expect("review meta should exist");
    meta.exit_code = 137;
    meta.primary_failure = Some("API key not found".to_string());
    csa_session::state::write_review_meta(&session_dir, &meta).expect("write review meta");

    let args = parse_review_args(&["csa", "review", "--check-verdict"]);
    let exit = handle_check_verdict(temp.path(), &args).unwrap();
    assert_eq!(exit, 1);
}

#[test]
fn issue_1716_check_verdict_rejects_unavailable_nonzero_with_empty_failure_metadata() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = setup_git_repo();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    run_git(temp.path(), &["checkout", "-b", "feature"]);
    let branch = run_git(temp.path(), &["branch", "--show-current"]);
    let head_sha = csa_session::detect_git_head(temp.path()).unwrap();

    let session_id = write_review_session(
        temp.path(),
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );
    let session_dir = csa_session::get_session_dir(temp.path(), &session_id).unwrap();
    let mut meta = read_review_meta(&session_dir)
        .unwrap()
        .expect("review meta should exist");
    meta.decision = ReviewDecision::Unavailable.as_str().to_string();
    meta.verdict = "UNAVAILABLE".to_string();
    meta.exit_code = 1;
    meta.status_reason = None;
    meta.primary_failure = None;
    meta.failure_reason = None;
    csa_session::state::write_review_meta(&session_dir, &meta).expect("write review meta");

    let args = parse_review_args(&["csa", "review", "--check-verdict"]);
    let exit = handle_check_verdict(temp.path(), &args).unwrap();
    assert_eq!(exit, 1);
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
        None,
    )
    .unwrap();
    assert!(found.is_none());
}

#[test]
fn check_verdict_explicit_session_does_not_fallback_to_stale_pass() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let branch = "feature";
    let head_sha = "abc1234567890";
    let stale_pass = write_review_session(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );
    let explicit_fail = write_review_session(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Fail,
        "REVISE",
    );
    set_review_timestamp(&project, &explicit_fail, 10);
    set_review_timestamp(&project, &stale_pass, 20);

    let explicit = check_review_verdict_for_session(
        &project,
        &explicit_fail,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .unwrap();
    assert!(
        explicit.is_none(),
        "explicit failed session must not fall back to another PASS session"
    );

    let scanned = check_review_verdict_for_target(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .unwrap()
    .expect("target scan should still find stale pass");
    assert_eq!(scanned.session_id, stale_pass);
}

#[test]
fn check_verdict_rejects_fix_pass_artifact_with_failed_meta_explicit_marker_and_scan() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let branch = "feature";
    let head_sha = "abc1234567890";
    let session_id = write_review_session(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );
    let session_dir = csa_session::get_session_dir(&project, &session_id).unwrap();
    let mut meta: ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap(),
    )
    .unwrap();
    meta.exit_code = 1;
    meta.fix_attempted = true;
    meta.fix_rounds = 3;
    meta.failure_reason = Some("fix_non_convergence:quality_gate_failed".to_string());
    meta.fix_convergence = Some(csa_session::FixConvergenceMeta {
        quality_gate_passed: false,
        fix_output_was_substantive: true,
        post_consistency_decision: ReviewDecision::Fail.as_str().to_string(),
        reached_genuine_clean_convergence: false,
        terminal_reason: "quality_gate_failed".to_string(),
    });
    csa_session::state::write_review_meta(&session_dir, &meta).expect("write failed fix meta");
    crate::review_gate::write_review_gate_marker(
        &project,
        branch,
        head_sha,
        &session_id,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    );

    let explicit = check_review_verdict_for_session(
        &project,
        &session_id,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .unwrap();
    assert!(
        explicit.is_none(),
        "explicit check must reject pass artifact when fix meta failed"
    );

    let scanned = check_review_verdict_for_target(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .unwrap();
    assert!(
        scanned.is_none(),
        "marker and session-scan paths must reject pass artifact when fix meta failed"
    );
}

#[test]
fn check_verdict_accepts_clean_initial_fix_without_fix_round_explicit_marker_and_scan() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let branch = "feature";
    let head_sha = "abc1234567890";
    let session_id = write_review_session(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );
    let session_dir = csa_session::get_session_dir(&project, &session_id).unwrap();
    let meta: ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap(),
    )
    .unwrap();
    assert!(!meta.fix_attempted);
    assert_eq!(meta.fix_rounds, 0);
    assert!(meta.fix_convergence.is_none());
    crate::review_gate::write_review_gate_marker(
        &project,
        branch,
        head_sha,
        &session_id,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    );

    let explicit = check_review_verdict_for_session(
        &project,
        &session_id,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .unwrap()
    .expect("explicit check should accept clean initial --fix metadata");
    assert_eq!(explicit.session_id, session_id);

    let scanned = check_review_verdict_for_target(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .unwrap()
    .expect("marker and session-scan paths should accept clean initial --fix metadata");
    assert_eq!(scanned.session_id, session_id);
}

#[test]
fn check_verdict_accepts_quota_unavailable_co_reviewer_clean_primary() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let branch = "feature";
    let head_sha = "abc1234567890";
    let session_id = write_review_session(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );
    let session_dir = csa_session::get_session_dir(&project, &session_id).unwrap();
    let mut meta: ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap(),
    )
    .unwrap();
    meta.primary_failure = Some("co-reviewer quota unavailable".to_string());
    csa_session::state::write_review_meta(&session_dir, &meta).expect("write quota meta");

    let explicit = check_review_verdict_for_session(
        &project,
        &session_id,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .unwrap()
    .expect("explicit check should accept clean primary despite co-reviewer quota");
    assert_eq!(explicit.session_id, session_id);

    let scanned = check_review_verdict_for_target(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .unwrap()
    .expect("scan should accept clean primary despite co-reviewer quota");
    assert_eq!(scanned.session_id, session_id);
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

// ---------------------------------------------------------------------------
// #1817: review-mode auditing for the merge gate. `--check-verdict` with an
// explicit `--red-team`/`--review-mode` requires the persisted verdict to carry
// the matching review mode; without a mode filter the gate keeps legacy behavior.
// ---------------------------------------------------------------------------

/// Stamp a review mode onto both the meta and the verdict artifact of an
/// already-written session (mirrors the production single-source flow where
/// `ReviewSessionMeta.review_mode` drives `ReviewVerdictArtifact.review_mode`).
fn set_review_mode(project_root: &Path, session_id: &str, review_mode: Option<&str>) {
    let session_dir = csa_session::get_session_dir(project_root, session_id).unwrap();
    let mut meta = read_review_meta(&session_dir)
        .unwrap()
        .expect("review meta should exist");
    meta.review_mode = review_mode.map(str::to_string);
    csa_session::state::write_review_meta(&session_dir, &meta).expect("write review meta");

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let raw = std::fs::read_to_string(&verdict_path).expect("read review verdict");
    let mut artifact: ReviewVerdictArtifact =
        serde_json::from_str(&raw).expect("parse review verdict");
    artifact.review_mode = review_mode.map(str::to_string);
    csa_session::write_review_verdict(&session_dir, &artifact).expect("write review verdict");
}

#[test]
fn required_check_verdict_mode_resolves_only_when_explicitly_requested() {
    // No mode flag → no requirement (legacy gate behavior preserved).
    let plain = parse_review_args(&["csa", "review", "--check-verdict", "--range", "main...HEAD"]);
    assert_eq!(required_check_verdict_mode(&plain), None);

    // `--red-team` shorthand → require red-team.
    let red_team = parse_review_args(&[
        "csa",
        "review",
        "--check-verdict",
        "--red-team",
        "--range",
        "main...HEAD",
    ]);
    assert_eq!(
        required_check_verdict_mode(&red_team).as_deref(),
        Some("red-team")
    );

    // Explicit `--review-mode red-team` → require red-team.
    let review_mode_flag = parse_review_args(&[
        "csa",
        "review",
        "--check-verdict",
        "--review-mode",
        "red-team",
        "--range",
        "main...HEAD",
    ]);
    assert_eq!(
        required_check_verdict_mode(&review_mode_flag).as_deref(),
        Some("red-team")
    );

    // Explicit `--review-mode standard` is still an explicit requirement.
    let standard_flag = parse_review_args(&[
        "csa",
        "review",
        "--check-verdict",
        "--review-mode",
        "standard",
        "--range",
        "main...HEAD",
    ]);
    assert_eq!(
        required_check_verdict_mode(&standard_flag).as_deref(),
        Some("standard")
    );
}

#[test]
fn review_mode_matches_treats_absent_requirement_as_wildcard() {
    // No requirement always matches (legacy/unfiltered gate).
    assert!(review_mode_matches(None, None));
    assert!(review_mode_matches(Some("red-team"), None));
    assert!(review_mode_matches(Some("standard"), None));

    // A requirement matches only an exact candidate; legacy `None` cannot prove it.
    assert!(review_mode_matches(Some("red-team"), Some("red-team")));
    assert!(!review_mode_matches(Some("standard"), Some("red-team")));
    assert!(!review_mode_matches(None, Some("red-team")));
}

#[test]
fn check_verdict_red_team_required_accepts_matching_red_team_verdict() {
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
    set_review_mode(&project, &session_id, Some("red-team"));

    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        Some("red-team"),
    )
    .unwrap()
    .expect("red-team verdict must satisfy a red-team requirement");
    assert_eq!(found.session_id, session_id);
}

#[test]
fn check_verdict_red_team_required_rejects_standard_verdict() {
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
    set_review_mode(&project, &session_id, Some("standard"));

    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        Some("red-team"),
    )
    .unwrap();
    assert!(
        found.is_none(),
        "a standard-mode verdict must not satisfy a red-team requirement"
    );
}

#[test]
fn check_verdict_red_team_required_rejects_legacy_verdict_without_mode() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    // Legacy session: review_mode is absent (written before #1817).
    let _session_id = write_review_session(
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
        Some("red-team"),
    )
    .unwrap();
    assert!(
        found.is_none(),
        "a legacy verdict with no recorded mode cannot prove a red-team review ran"
    );
}

#[test]
fn check_verdict_without_mode_filter_accepts_any_mode() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    // Legacy (no mode) verdict is accepted by an unfiltered gate, byte-for-byte.
    let legacy_id = write_review_session(
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
        None,
    )
    .unwrap()
    .expect("unfiltered gate must accept a legacy verdict");
    assert_eq!(found.session_id, legacy_id);

    // A red-team verdict is likewise accepted by an unfiltered gate.
    set_review_mode(&project, &legacy_id, Some("red-team"));
    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .unwrap()
    .expect("unfiltered gate must accept a red-team verdict");
    assert_eq!(found.session_id, legacy_id);
}

#[test]
fn check_verdict_red_team_marker_fast_path_enforces_mode() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let branch = "feature";
    let head_sha = "abcdef1234567890";
    let session_id = write_review_session(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );
    set_review_mode(&project, &session_id, Some("red-team"));
    crate::review_gate::write_review_gate_marker(
        &project,
        branch,
        head_sha,
        &session_id,
        REQUIRED_FULL_DIFF_SCOPE,
        Some("red-team"),
    );

    // Force the marker to be the only locator (session branch no longer matches).
    set_session_branch(&project, &session_id, "other-branch");

    let found = check_review_verdict_for_target(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        Some("red-team"),
    )
    .unwrap()
    .expect("red-team marker must satisfy a red-team requirement");
    assert_eq!(found.session_id, session_id);
}

#[test]
fn check_verdict_red_team_required_rejects_legacy_marker() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let branch = "feature";
    let head_sha = "abcdef1234567890";
    let session_id = write_review_session(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        ReviewDecision::Pass,
        "CLEAN",
    );
    // Legacy marker + legacy session: neither records a review mode.
    crate::review_gate::write_review_gate_marker(
        &project,
        branch,
        head_sha,
        &session_id,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    );

    let found = check_review_verdict_for_target(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        Some("red-team"),
    )
    .unwrap();
    assert!(
        found.is_none(),
        "a legacy marker without a recorded mode must not satisfy a red-team requirement"
    );
}
