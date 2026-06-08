use super::*;
use std::path::Path;

fn run_test_git(project_root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .expect("git command should start");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_recovery_test_repo(project_root: &Path, track_weave_lock: bool) {
    std::fs::create_dir_all(project_root).expect("repo dir should be created");
    run_test_git(project_root, &["init", "-b", "main"]);
    run_test_git(
        project_root,
        &["config", "user.email", "csa-test@example.com"],
    );
    run_test_git(project_root, &["config", "user.name", "CSA Test"]);
    run_test_git(project_root, &["config", "core.excludesFile", "/dev/null"]);
    std::fs::write(project_root.join("README.md"), "test repo\n").expect("write readme");
    if track_weave_lock {
        std::fs::write(project_root.join(WEAVE_LOCK), "lock = 1\n").expect("write weave.lock");
        run_test_git(project_root, &["add", "README.md", WEAVE_LOCK]);
    } else {
        run_test_git(project_root, &["add", "README.md"]);
    }
    run_test_git(project_root, &["commit", "-m", "initial"]);
}

fn current_branch(project_root: &Path) -> String {
    run_git(project_root, &["branch", "--show-current"]).expect("branch should resolve")
}

#[test]
fn persisted_failure_output_redacts_step_secrets() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let session_dir = temp.path().join("session");
    let workflow_path = temp.path().join("workflow.toml");
    let results = vec![StepResult {
        step_id: 1,
        title: "Secret Failure".to_string(),
        exit_code: 7,
        duration_secs: 0.0,
        skipped: false,
        error: Some("Exit code 7\nstderr:\npassword=hunter2".to_string()),
        output: None,
        session_id: None,
        command: Some(
            "curl -H 'Authorization: Bearer abcDEF123._-token' api_key=key-prod_987654321"
                .to_string(),
        ),
        stderr: Some("client_secret=top-secret-value".to_string()),
    }];
    let report = PlanFailureReport::from_results(
        "failing-plan",
        &workflow_path,
        "1 step(s) failed".to_string(),
        &results,
        None,
    );

    persist_plan_failure_output(&session_dir, &report).expect("failure output should persist");

    let output_log =
        std::fs::read_to_string(session_dir.join("output.log")).expect("output.log should exist");
    let details = csa_session::read_section(&session_dir, "details")
        .expect("details should load")
        .expect("details section should exist");
    for rendered in [&output_log, &details] {
        assert!(
            rendered.contains("[REDACTED]"),
            "persisted failure output must mark redacted secrets: {rendered}"
        );
        assert!(
            !rendered.contains("abcDEF123._-token"),
            "bearer token leaked: {rendered}"
        );
        assert!(
            !rendered.contains("key-prod_987654321"),
            "api key leaked: {rendered}"
        );
        assert!(!rendered.contains("hunter2"), "password leaked: {rendered}");
        assert!(
            !rendered.contains("top-secret-value"),
            "client secret leaked: {rendered}"
        );
    }
}

#[test]
fn recovery_preserves_tracked_weave_lock_change_after_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let project_root = temp.path().join("repo");
    init_recovery_test_repo(&project_root, true);
    run_test_git(&project_root, &["switch", "-c", "fix/recovery"]);
    let snapshot = PlanFailureRecoverySnapshot::capture(&project_root);

    run_test_git(&project_root, &["switch", "main"]);
    std::fs::write(
        project_root.join(WEAVE_LOCK),
        "lock = 1\nuser concurrent edit\n",
    )
    .expect("write post-snapshot weave.lock change");

    let report = snapshot.recover_after_failure(&project_root);

    assert_eq!(report.status.as_str(), "manual-required");
    assert_eq!(current_branch(&project_root), "fix/recovery");
    assert!(
        std::fs::read_to_string(project_root.join(WEAVE_LOCK))
            .expect("weave.lock should remain")
            .contains("user concurrent edit"),
        "recovery must preserve tracked weave.lock content"
    );
    assert!(
        report
            .messages
            .iter()
            .any(|message| message.contains("Preserved dirty weave.lock")),
        "manual recovery message should explain why weave.lock was preserved: {report:?}"
    );
    assert!(
        report
            .final_status
            .iter()
            .any(|line| line.contains(WEAVE_LOCK)),
        "manual report should surface remaining weave.lock status: {report:?}"
    );
}

#[test]
fn recovery_preserves_untracked_weave_lock_change_after_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let project_root = temp.path().join("repo");
    init_recovery_test_repo(&project_root, false);
    let snapshot = PlanFailureRecoverySnapshot::capture(&project_root);
    std::fs::write(project_root.join(WEAVE_LOCK), "user concurrent edit\n")
        .expect("write untracked weave.lock");

    let report = snapshot.recover_after_failure(&project_root);

    assert_eq!(report.status.as_str(), "manual-required");
    assert_eq!(current_branch(&project_root), "main");
    assert_eq!(
        std::fs::read_to_string(project_root.join(WEAVE_LOCK))
            .expect("untracked weave.lock should remain"),
        "user concurrent edit\n"
    );
    assert!(
        report
            .final_status
            .iter()
            .any(|line| line.starts_with("?? ") && line.contains(WEAVE_LOCK)),
        "manual report should surface untracked weave.lock status: {report:?}"
    );
}

#[test]
fn recovery_commands_do_not_restore_initially_dirty_weave_lock() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let project_root = temp.path().join("repo");
    init_recovery_test_repo(&project_root, true);
    std::fs::write(
        project_root.join(WEAVE_LOCK),
        "lock = 1\npre-existing user edit\n",
    )
    .expect("write pre-existing weave.lock change");
    let snapshot = PlanFailureRecoverySnapshot::capture(&project_root);

    let report = snapshot.recover_after_failure(&project_root);

    assert_eq!(report.status.as_str(), "manual-required");
    assert!(
        report
            .messages
            .iter()
            .any(|message| message.contains("already dirty before pr-bot started")),
        "manual report should explain pre-existing dirty state: {report:?}"
    );
    assert!(
        report
            .recovery_commands
            .iter()
            .all(|command| !command.contains("git restore --staged --worktree -- weave.lock")),
        "manual recovery commands must not discard pre-existing weave.lock edits: {report:?}"
    );
    assert!(
        std::fs::read_to_string(project_root.join(WEAVE_LOCK))
            .expect("weave.lock should remain")
            .contains("pre-existing user edit"),
        "recovery report generation must not alter pre-existing weave.lock content"
    );
}
