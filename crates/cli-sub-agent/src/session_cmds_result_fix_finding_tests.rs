use std::process::Command;

use csa_session::{TaskContext, save_session};

use super::*;

const FIX_FINDING_TASK_TYPE: &str = "review_fix_finding";

fn run_git(project_root: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_git_repo(project_root: &std::path::Path) {
    run_git(project_root, &["init", "-q"]);
    run_git(
        project_root,
        &["config", "user.email", "csa-test@example.com"],
    );
    run_git(project_root, &["config", "user.name", "CSA Test"]);
    run_git(project_root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(project_root.join("tracked.txt"), "initial\n").unwrap();
    run_git(project_root, &["add", "tracked.txt"]);
    run_git(project_root, &["commit", "-q", "-m", "initial"]);
}

#[cfg(unix)]
#[test]
fn handle_session_result_on_fix_finding_reports_fix_session_missing_result() {
    let tmp = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = tmp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", tmp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = tmp.path();
    init_git_repo(project);

    let original_review = csa_session::create_session_fresh(
        project,
        Some("original failed review"),
        None,
        Some("codex"),
    )
    .unwrap();
    let original_review_id = original_review.meta_session_id;
    let mut fix_session = csa_session::create_session_fresh(
        project,
        Some("fix finding from review"),
        Some(&original_review_id),
        Some("codex"),
    )
    .unwrap();
    fix_session.task_context = TaskContext {
        task_type: Some(FIX_FINDING_TASK_TYPE.to_string()),
        tier_name: None,
    };
    save_session(&fix_session).unwrap();
    let fix_session_id = fix_session.meta_session_id;
    let fix_session_dir = get_session_dir(project, &fix_session_id).unwrap();
    backdate_tree(&fix_session_dir, 120);

    std::fs::write(project.join("tracked.txt"), "fixed but not recorded\n").unwrap();

    handle_session_result(
        fix_session_id.clone(),
        false,
        Some(project.to_string_lossy().into_owned()),
        StructuredOutputOpts::default(),
    )
    .unwrap();

    let result = load_result(project, &fix_session_id)
        .unwrap()
        .expect("session result should synthesize fix-finding diagnostics");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("fix-finding"), "{}", result.summary);
    assert!(
        result
            .summary
            .contains("original failed review verdict is not a fix-session result"),
        "{}",
        result.summary
    );
    assert!(
        result
            .summary
            .contains("repo_side_effects=dirty_or_committed_tracked_changes"),
        "{}",
        result.summary
    );
    assert!(
        result.summary.contains("modified=[tracked.txt]"),
        "{}",
        result.summary
    );
}
