use std::path::Path;
use std::process::Command;

use super::*;

#[tokio::test]
async fn require_commit_recovery_uses_raw_exit_after_incidental_downgrade() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let mut session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let session_dir = csa_session::get_session_dir(root, &session.meta_session_id)
        .expect("session dir should exist");
    let changed_paths = vec!["src.rs".to_string()];
    std::fs::write(root.join("src.rs"), "clean\n").expect("tracked file");
    run_git(root, &["add", "src.rs"]);
    run_git(root, &["commit", "-q", "-m", "track src"]);
    std::fs::write(root.join("src.rs"), "dirty\n").expect("dirty tracked file");

    let executor = csa_executor::Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: csa_executor::CodexRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let post_exec_ctx = crate::pipeline_post_exec::PostExecContext {
        executor: &executor,
        prompt: "write src.rs",
        effective_prompt: "write src.rs",
        task_type: Some("run"),
        readonly_project_root: false,
        project_root: root,
        config: None,
        global_config: None,
        session_dir,
        sessions_root: "test-root".to_string(),
        execution_start_time: chrono::Utc::now() - chrono::Duration::seconds(5),
        hooks_config: &hooks_config,
        memory_project_key: None,
        provider_session_id: None,
        events_count: 1,
        transcript_artifacts: Vec::new(),
        changed_paths: changed_paths.clone(),
        pre_exec_snapshot: None,
        timeout_diagnostics: None,
        has_tool_calls: true,
        turn_count: 1,
        output_tokens: None,
        sa_mode: false,
    };
    let mut execution = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "completed with dirty changes".to_string(),
        exit_code: 2,
        raw_process_exit_code: Some(2),
        model_completed: Some(true),
        terminal_reason: Some("end_turn".to_string()),
        ..Default::default()
    };

    crate::pipeline_post_exec::process_execution_result(
        post_exec_ctx,
        &mut session,
        &mut execution,
    )
    .await
    .expect("post-exec should downgrade the incidental raw exit");

    let downgraded = csa_session::load_result(root, &session.meta_session_id)
        .expect("load downgraded result")
        .expect("downgraded result should exist");
    assert_eq!(execution.exit_code, 0);
    assert_eq!(downgraded.status, "success");
    assert_eq!(downgraded.exit_code, 0);
    assert_eq!(downgraded.raw_process_exit_code, Some(2));
    assert!(
        downgraded
            .warnings
            .iter()
            .any(|warning| warning.contains("incidental nonzero exit (2)")),
        "downgrade warning should record the raw exit: {:?}",
        downgraded.warnings
    );

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: true,
            changed_paths: Some(&changed_paths),
            commit_created: Some(false),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    let loaded = csa_session::load_result(root, &session.meta_session_id)
        .expect("load require-commit result")
        .expect("require-commit result should exist");
    assert_eq!(execution.exit_code, 1);
    assert_eq!(
        execution.csa_gate_failure.as_deref(),
        Some("writer-uncommitted")
    );
    assert_eq!(loaded.status, "failure");
    assert_eq!(loaded.exit_code, 1);
    assert!(
        loaded.warnings.is_empty(),
        "fatal contract result must not keep success warnings: {:?}",
        loaded.warnings
    );
    assert_eq!(loaded.raw_process_exit_code, Some(2));
    let recovery = loaded
        .require_commit_recovery
        .expect("require-commit recovery should be recorded");
    assert_eq!(recovery.termination_status, "failure");
    assert_eq!(recovery.exit_code, 2);
    assert_eq!(recovery.termination_signal, None);
    assert_eq!(recovery.changed_paths, vec!["src.rs".to_string()]);
}

fn init_repo_with_initial_commit() -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    run_git(root, &["init", "-q"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test User"]);
    run_git(root, &["config", "commit.gpgsign", "false"]);
    run_git(
        root,
        &["config", "core.hooksPath", "/nonexistent-csa-hooks"],
    );
    std::fs::write(root.join("seed.txt"), "seed\n").expect("write seed");
    run_git(root, &["add", "seed.txt"]);
    run_git(root, &["commit", "-q", "-m", "initial"]);
    temp
}

fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git command should execute");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}
