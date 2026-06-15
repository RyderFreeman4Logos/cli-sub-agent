use super::*;
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
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
