use super::*;
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use chrono::{TimeZone, Utc};
use csa_core::types::ReviewDecision;
use csa_core::vcs::{VcsIdentity, VcsKind};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

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

fn setup_git_repo_with_feature_diff(project: &Path) -> (String, String) {
    std::fs::create_dir_all(project).expect("create project dir");
    run_git(project, &["init"]);
    run_git(project, &["config", "user.email", "test@example.com"]);
    run_git(project, &["config", "user.name", "Test User"]);

    std::fs::write(project.join("tracked.txt"), "baseline\n").expect("write baseline");
    run_git(project, &["add", "tracked.txt"]);
    run_git(project, &["commit", "-m", "initial"]);
    run_git(project, &["branch", "-M", "main"]);

    run_git(project, &["checkout", "-b", "feature"]);
    std::fs::write(project.join("tracked.txt"), "baseline\nfeature change\n")
        .expect("write feature change");
    run_git(project, &["add", "tracked.txt"]);
    run_git(project, &["commit", "-m", "feature change"]);

    let branch = run_git(project, &["branch", "--show-current"]);
    let head_sha = run_git(project, &["rev-parse", "HEAD"]);
    assert_eq!(branch, "feature");
    (branch, head_sha)
}

fn timestamp(seconds: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(seconds, 0).single().unwrap()
}

#[test]
fn daemon_completion_before_result_preserves_exact_head_review_availability_for_non_empty_diff() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    let (branch, head_sha) = setup_git_repo_with_feature_diff(&project);
    let expected_diff_fingerprint =
        crate::review_cmd::compute_review_diff_fingerprint(&project, REQUIRED_FULL_DIFF_SCOPE)
            .expect("feature branch should have a non-empty main...HEAD diff");

    let mut session = csa_session::create_session_fresh(
        &project,
        Some("review: range:main...HEAD"),
        None,
        Some("codex"),
    )
    .expect("create daemon review session");
    session.branch = Some(branch.clone());
    session.git_head_at_creation = Some(head_sha.clone());
    session.vcs_identity = Some(VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some(head_sha.clone()),
        change_id: None,
        short_id: Some(head_sha.chars().take(12).collect()),
        ref_name: Some(branch.clone()),
        op_id: None,
    });
    session.task_context = csa_session::TaskContext {
        task_type: Some("review".to_string()),
        tier_name: None,
    };
    csa_session::save_session(&session).expect("save daemon review session state");
    let session_id = session.meta_session_id.clone();
    let session_dir = csa_session::get_session_dir(&project, &session_id).unwrap();
    assert!(
        !session_dir.join("review_meta.json").exists(),
        "regression setup should recover from artifact-only verdict state"
    );
    csa_session::write_review_verdict(
        &session_dir,
        &ReviewVerdictArtifact::from_parts(
            session_id.clone(),
            ReviewDecision::Pass,
            "CLEAN",
            &[],
            Vec::new(),
        ),
    )
    .expect("write review verdict before result.toml");

    let _daemon_id = ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_ID", &session_id);
    let _daemon_dir = ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_DIR", &session_dir);
    let _daemon_project = ScopedEnvVarRestore::set("CSA_DAEMON_PROJECT_ROOT", &project);
    crate::session_cmds_daemon::persist_daemon_completion_from_env(1);

    let found = check_review_verdict_for_target(
        &project,
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        Some(expected_diff_fingerprint.as_str()),
        None,
    )
    .unwrap()
    .expect("existing review verdict should remain available for exact-head gates");
    assert_eq!(found.session_id, session_id);

    let recovered_meta: ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("review_meta.json"))
            .expect("daemon completion should recover review_meta.json"),
    )
    .expect("recovered review_meta.json should parse");
    assert_eq!(
        recovered_meta.diff_fingerprint.as_deref(),
        Some(expected_diff_fingerprint.as_str())
    );

    let result = csa_session::load_result(&project, &session_id)
        .unwrap()
        .expect("daemon completion should publish a result from review artifacts");
    assert_eq!(result.status, "success");
    assert_eq!(result.exit_code, 0);
}

#[test]
fn recovered_artifact_only_pass_keeps_original_timestamp_so_newer_fail_blocks_gate() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    let (branch, head_sha) = setup_git_repo_with_feature_diff(&project);
    let expected_diff_fingerprint =
        crate::review_cmd::compute_review_diff_fingerprint(&project, REQUIRED_FULL_DIFF_SCOPE)
            .expect("feature branch should have a non-empty main...HEAD diff");

    let mut older_pass_session = csa_session::create_session_fresh(
        &project,
        Some("review: range:main...HEAD"),
        None,
        Some("codex"),
    )
    .expect("create artifact-only daemon review session");
    older_pass_session.branch = Some(branch.clone());
    older_pass_session.git_head_at_creation = Some(head_sha.clone());
    older_pass_session.vcs_identity = Some(VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some(head_sha.clone()),
        change_id: None,
        short_id: Some(head_sha.chars().take(12).collect()),
        ref_name: Some(branch.clone()),
        op_id: None,
    });
    older_pass_session.task_context = csa_session::TaskContext {
        task_type: Some("review".to_string()),
        tier_name: None,
    };
    csa_session::save_session(&older_pass_session).expect("save artifact-only session state");
    let older_pass_id = older_pass_session.meta_session_id.clone();
    let older_pass_dir = csa_session::get_session_dir(&project, &older_pass_id).unwrap();
    let older_pass_time = timestamp(1_000);
    let mut older_pass_verdict = ReviewVerdictArtifact::from_parts(
        older_pass_id.clone(),
        ReviewDecision::Pass,
        "CLEAN",
        &[],
        Vec::new(),
    );
    older_pass_verdict.timestamp = older_pass_time;
    csa_session::write_review_verdict(&older_pass_dir, &older_pass_verdict)
        .expect("write artifact-only pass verdict");
    assert!(
        !older_pass_dir.join("review_meta.json").exists(),
        "regression setup should start with an artifact-only PASS"
    );

    let mut newer_fail_session = csa_session::create_session_fresh(
        &project,
        Some("review: range:main...HEAD"),
        None,
        Some("codex"),
    )
    .expect("create newer failing review session");
    newer_fail_session.branch = Some(branch.clone());
    newer_fail_session.git_head_at_creation = Some(head_sha.clone());
    newer_fail_session.vcs_identity = Some(VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some(head_sha.clone()),
        change_id: None,
        short_id: Some(head_sha.chars().take(12).collect()),
        ref_name: Some(branch.clone()),
        op_id: None,
    });
    csa_session::save_session(&newer_fail_session).expect("save newer failing session state");
    let newer_fail_id = newer_fail_session.meta_session_id.clone();
    let newer_fail_dir = csa_session::get_session_dir(&project, &newer_fail_id).unwrap();
    let newer_fail_time = timestamp(2_000);
    let newer_fail_meta = ReviewSessionMeta {
        session_id: newer_fail_id.clone(),
        head_sha: head_sha.clone(),
        decision: ReviewDecision::Fail.as_str().to_string(),
        verdict: "HAS_ISSUES".to_string(),
        review_mode: None,
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "codex".to_string(),
        scope: REQUIRED_FULL_DIFF_SCOPE.to_string(),
        exit_code: 1,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: newer_fail_time,
        diff_fingerprint: Some(expected_diff_fingerprint.clone()),
        fix_convergence: None,
    };
    csa_session::state::write_review_meta(&newer_fail_dir, &newer_fail_meta)
        .expect("write newer failing review meta");
    let mut newer_fail_verdict = ReviewVerdictArtifact::from_parts(
        newer_fail_id,
        ReviewDecision::Fail,
        "HAS_ISSUES",
        &[],
        Vec::new(),
    );
    newer_fail_verdict.timestamp = newer_fail_time;
    csa_session::write_review_verdict(&newer_fail_dir, &newer_fail_verdict)
        .expect("write newer failing review verdict");

    let _daemon_id = ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_ID", &older_pass_id);
    let _daemon_dir = ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_DIR", &older_pass_dir);
    let _daemon_project = ScopedEnvVarRestore::set("CSA_DAEMON_PROJECT_ROOT", &project);
    crate::session_cmds_daemon::persist_daemon_completion_from_env(1);

    let recovered_meta: ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(older_pass_dir.join("review_meta.json"))
            .expect("daemon completion should recover older PASS review_meta.json"),
    )
    .expect("recovered older PASS review_meta.json should parse");
    assert_eq!(recovered_meta.timestamp, older_pass_time);
    assert_eq!(
        recovered_meta.diff_fingerprint.as_deref(),
        Some(expected_diff_fingerprint.as_str())
    );

    let found = check_review_verdict_for_target(
        &project,
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        Some(expected_diff_fingerprint.as_str()),
        None,
    )
    .unwrap();
    assert!(
        found.is_none(),
        "a recovered older PASS must not outrank a newer FAIL for the same branch/head/scope/diff"
    );
}
