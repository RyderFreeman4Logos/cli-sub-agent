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

fn write_plan_journal(project_root: &Path, content: &str) {
    let journal_path = project_root.join(".csa/state/plan/pr-bot.journal.json");
    std::fs::create_dir_all(
        journal_path
            .parent()
            .expect("journal path should have parent"),
    )
    .expect("plan state dir should be created");
    std::fs::write(journal_path, content).expect("plan journal should be written");
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
fn failure_summary_surfaces_actionable_step_stderr() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let workflow_path = temp.path().join("workflow.toml");
    let results = vec![StepResult {
        step_id: 7,
        title: "Plan with mktd".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some("Exit code 1".to_string()),
        output: None,
        session_id: None,
        command: Some("csa plan run patterns/mktd/workflow.toml".to_string()),
        stderr: Some(
            [
                "✓ PASS   Step 7 - Phase 2 — DRAFT TODO",
                "✗ FAIL   Step 13 - Save TODO (0.02s) — Exit code 2",
                "ERROR: TODO artifact has an open task without a mechanically-verifiable DONE WHEN: clause.",
                "Error: 1 step(s) failed (1 execution, 0 unsupported-skip)",
            ]
            .join("\n"),
        ),
    }];
    let report = PlanFailureReport::from_results(
        "dev2merge",
        &workflow_path,
        "1 step(s) failed".to_string(),
        &results,
        None,
    );

    let summary_line = report.summary_line("patterns/dev2merge/workflow.toml");
    let summary_section = report.render_summary_section();

    assert!(
        summary_line.contains("detail=ERROR: TODO artifact has an open task"),
        "result summary should include actionable stderr detail: {summary_line}"
    );
    assert!(
        summary_section.contains("Failure detail: ERROR: TODO artifact has an open task"),
        "structured summary section should include actionable stderr detail: {summary_section}"
    );
}

#[test]
fn failure_summary_surfaces_mktd_stdout_for_post_2082_issue_body_shape() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let workflow_path = temp.path().join("workflow.toml");
    let results = vec![StepResult {
        step_id: 7,
        title: "Plan with mktd".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some(
            [
                "Exit code 1",
                "stderr (last 20 lines):",
                "ERROR: mktd did not produce a TODO for branch fix/2086-require-commit-openai-fallback.",
                "stdout (last 20 lines):",
                "## Command",
                "```bash",
                "csa run --tool codex --tier tier-2-standard --allow-fallback --require-commit --prompt-file /dev/stdin",
                "```",
                "## Observed",
                "```text",
                "WARNING: csa run completed but run left uncommitted workspace mutations compared to start.",
                "ERROR csa::pipeline: Tool 'openai-compat' is not installed.",
                "Error: Tool 'openai-compat' is not installed or not in PATH",
                "```",
                "Error: 1 step(s) failed (1 execution, 0 unsupported-skip)",
            ]
            .join("\n"),
        ),
        output: None,
        session_id: None,
        command: Some("timeout -k 30 1800 csa plan run --sa-mode true patterns/mktd/workflow.toml --var FEATURE='Plan dev2merge for branch fix/2086'".to_string()),
        stderr: Some(
            "ERROR: mktd did not produce a TODO for branch fix/2086-require-commit-openai-fallback."
                .to_string(),
        ),
    }];
    let report = PlanFailureReport::from_results(
        "dev2merge",
        &workflow_path,
        "1 step(s) failed".to_string(),
        &results,
        None,
    );

    let summary_line = report.summary_line("patterns/dev2merge/workflow.toml");
    let summary_section = report.render_summary_section();
    let details_section = report.render_details_section();

    assert!(
        summary_line.contains("detail=Error: Tool 'openai-compat' is not installed or not in PATH"),
        "result summary should prefer concrete child mktd stdout over generic Step 7 gate text: {summary_line}"
    );
    assert!(
        summary_section.contains(
            "Failure detail: Error: Tool 'openai-compat' is not installed or not in PATH"
        ),
        "structured summary should expose concrete child mktd stdout detail: {summary_section}"
    );
    assert!(
        summary_section.contains("Recovery hint: Inspect the mktd failure detail above"),
        "structured summary should include an actionable mktd recovery hint: {summary_section}"
    );
    assert!(
        details_section.contains("stdout (last 20 lines):")
            && details_section.contains("--prompt-file /dev/stdin")
            && details_section.contains("openai-compat"),
        "details should preserve the post-#2082 issue-body shape and child failure context: {details_section}"
    );
}

#[test]
fn recovery_ignores_untracked_plan_journal_after_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let project_root = temp.path().join("repo");
    init_recovery_test_repo(&project_root, false);
    run_test_git(&project_root, &["switch", "-c", "fix/recovery"]);
    let snapshot = PlanFailureRecoverySnapshot::capture(&project_root);

    run_test_git(&project_root, &["switch", "main"]);
    write_plan_journal(&project_root, r#"{"status":"running"}"#);

    let report = snapshot.recover_after_failure(&project_root);

    assert_eq!(report.status.as_str(), "restored");
    assert_eq!(current_branch(&project_root), "fix/recovery");
    assert!(
        report
            .final_status
            .iter()
            .all(|line| !line.contains(".csa/")),
        "CSA plan state should not appear as remaining recovery dirt: {report:?}"
    );
}

#[test]
fn recovery_ignores_tracked_plan_journal_change_after_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let project_root = temp.path().join("repo");
    init_recovery_test_repo(&project_root, false);
    write_plan_journal(&project_root, r#"{"status":"running"}"#);
    run_test_git(
        &project_root,
        &["add", ".csa/state/plan/pr-bot.journal.json"],
    );
    run_test_git(&project_root, &["commit", "-m", "track plan journal"]);
    run_test_git(&project_root, &["switch", "-c", "fix/recovery"]);
    let snapshot = PlanFailureRecoverySnapshot::capture(&project_root);

    run_test_git(&project_root, &["switch", "main"]);
    write_plan_journal(&project_root, r#"{"status":"failed"}"#);

    let report = snapshot.recover_after_failure(&project_root);

    assert_eq!(report.status.as_str(), "restored");
    assert_eq!(current_branch(&project_root), "fix/recovery");
    assert!(
        std::fs::read_to_string(project_root.join(".csa/state/plan/pr-bot.journal.json"))
            .expect("plan journal should remain readable")
            .contains("failed"),
        "recovery must not discard tracked CSA plan journal content"
    );
    assert!(
        report
            .final_status
            .iter()
            .all(|line| !line.contains(".csa/")),
        "tracked CSA plan journal changes should not appear as remaining recovery dirt: {report:?}"
    );
}

#[test]
fn recovery_preserves_unknown_csa_file_after_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let project_root = temp.path().join("repo");
    init_recovery_test_repo(&project_root, false);
    run_test_git(&project_root, &["switch", "-c", "fix/recovery"]);
    let snapshot = PlanFailureRecoverySnapshot::capture(&project_root);

    run_test_git(&project_root, &["switch", "main"]);
    std::fs::create_dir_all(project_root.join(".csa")).expect("CSA dir should be created");
    std::fs::write(project_root.join(".csa/config.toml"), "tool = 'codex'\n")
        .expect("CSA config should be written");

    let report = snapshot.recover_after_failure(&project_root);

    assert_eq!(report.status.as_str(), "manual-required");
    assert_eq!(
        current_branch(&project_root),
        "main",
        "unknown .csa files must still block automatic checkout recovery"
    );
    assert!(
        report
            .messages
            .iter()
            .any(|message| message.contains(".csa/config.toml")),
        "manual report should surface unknown .csa dirt: {report:?}"
    );
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
