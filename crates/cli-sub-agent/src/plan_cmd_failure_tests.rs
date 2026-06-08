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
