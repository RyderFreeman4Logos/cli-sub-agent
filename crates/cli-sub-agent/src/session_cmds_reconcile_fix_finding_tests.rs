use super::*;

use csa_session::{TaskContext, save_session};

#[cfg(unix)]
#[test]
fn fix_finding_missing_result_with_unpushed_commits_suppresses_push_next_step() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path().join("project");
    let origin = td.path().join("origin.git");
    fs::create_dir_all(&project).unwrap();

    run_git(&project, &["init", "--initial-branch", "main"]);
    run_git(&project, &["config", "user.email", "test@example.com"]);
    run_git(&project, &["config", "user.name", "Test User"]);
    fs::write(project.join("README.md"), "base\n").unwrap();
    run_git(&project, &["add", "README.md"]);
    run_git(&project, &["commit", "-m", "init"]);

    run_git(td.path(), &["init", "--bare", origin.to_str().unwrap()]);
    run_git(
        &project,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    run_git(&project, &["push", "-u", "origin", "main"]);

    run_git(&project, &["checkout", "-b", "fix/finding-side-effects"]);
    let mut session = create_session(
        &project,
        Some("fix finding from review"),
        None,
        Some("codex"),
    )
    .unwrap();
    session.task_context = TaskContext {
        task_type: Some("review_fix_finding".to_string()),
        tier_name: None,
    };
    save_session(&session).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(&project, &session_id).unwrap();

    fs::write(project.join("committed-fix.txt"), "committed\n").unwrap();
    run_git(&project, &["add", "committed-fix.txt"]);
    run_git(&project, &["commit", "-m", "fix: committed side effect"]);
    fs::write(project.join("staged-fix.txt"), "staged\n").unwrap();
    run_git(&project, &["add", "staged-fix.txt"]);
    tail_backdate_tree(&session_dir, 120);

    let reconciled =
        ensure_terminal_result_for_dead_active_session(&project, &session_id, "session wait")
            .unwrap();
    assert_eq!(
        reconciled,
        DeadActiveSessionReconciliation::SynthesizedFailure
    );

    let unpushed_sidecar: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(session_dir.join("output").join("unpushed_commits.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        unpushed_sidecar["recovery_command"],
        "git push -u origin fix/finding-side-effects"
    );

    let recovery_sidecar_path =
        crate::session_fix_finding_recovery::recovery_sidecar_path(&session_dir);
    let recovery_sidecar: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&recovery_sidecar_path).unwrap()).unwrap();
    assert_eq!(
        recovery_sidecar["outcome"],
        serde_json::json!("failed_closed_missing_result")
    );
    assert_eq!(
        recovery_sidecar["allow_required_push_next_step"],
        serde_json::json!(false)
    );
    assert_eq!(
        recovery_sidecar["requires_fresh_exact_head_review"],
        serde_json::json!(true)
    );
    assert_eq!(
        recovery_sidecar["side_effects"]["status"],
        serde_json::json!("dirty_or_committed_tracked_changes")
    );

    assert!(
        crate::session_cmds_daemon::synthesized_wait_next_step(&session_dir)
            .unwrap()
            .is_none(),
        "failed-closed fix-finding recovery must not synthesize a required git push next-step"
    );

    let result = load_result(&project, &session_id).unwrap().unwrap();
    let wait_summary =
        crate::session_cmds_daemon::render_wait_result_summary(&session_dir, &session_id, &result);
    assert!(wait_summary.contains("required push next-step suppressed"));
    assert!(wait_summary.contains("hook-enabled commit"));
    assert!(wait_summary.contains("fresh exact-head review before push/PR"));
}
