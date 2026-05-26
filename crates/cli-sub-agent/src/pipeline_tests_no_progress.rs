//! Tests for post-session no-progress classification in `process_execution_result`.

use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_executor::{CodexRuntimeMetadata, Executor};
use csa_session::{SessionResult, create_session, load_result, load_session};

fn build_test_ctx<'a>(
    executor: &'a Executor,
    session_dir: std::path::PathBuf,
    project_root: &'a std::path::Path,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    hooks_config: &'a csa_hooks::HooksConfig,
) -> PostExecContext<'a> {
    PostExecContext {
        executor,
        prompt: "test prompt",
        effective_prompt: "test prompt",
        task_type: Some("run"),
        readonly_project_root: false,
        project_root,
        config: None,
        global_config: None,
        session_dir,
        sessions_root: "test-root".to_string(),
        execution_start_time,
        hooks_config,
        memory_project_key: None,
        provider_session_id: None,
        events_count: 4,
        transcript_artifacts: vec![],
        changed_paths: vec![],
        pre_exec_snapshot: None,
        has_tool_calls: true,
        turn_count: 0,
        output_tokens: None,
        sa_mode: false,
    }
}

fn build_test_result(summary: &str) -> csa_process::ExecutionResult {
    csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: summary.to_string(),
        exit_code: 0,
        peak_memory_mb: None,
    }
}

fn build_codex_executor() -> Executor {
    Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::current(),
    }
}

fn write_result_sidecar(session_dir: &std::path::Path, contents: &str) {
    let path = session_dir.join(csa_session::CONTRACT_RESULT_ARTIFACT_PATH);
    std::fs::create_dir_all(path.parent().expect("sidecar parent")).expect("create output dir");
    std::fs::write(path, contents).expect("write result sidecar");
}

fn run_git(repo: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|err| panic!("spawn git {args:?}: {err}"));
    assert!(
        output.status.success(),
        "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn setup_git_repo(repo: &std::path::Path) {
    run_git(repo, &["init"]);
    run_git(repo, &["config", "user.email", "csa-test@example.com"]);
    run_git(repo, &["config", "user.name", "CSA Test"]);
    std::fs::write(repo.join("tracked.txt"), "initial\n").expect("write tracked file");
    run_git(repo, &["add", "tracked.txt"]);
    run_git(repo, &["commit", "-m", "initial"]);
}

fn setup_session_repo(
    tmp: &tempfile::TempDir,
) -> (std::path::PathBuf, csa_session::MetaSessionState) {
    let project_root = tmp.path().join("repo");
    std::fs::create_dir_all(&project_root).expect("create project root");
    setup_git_repo(&project_root);
    let session =
        create_session(&project_root, Some("test"), None, Some("codex")).expect("create session");
    (project_root, session)
}

#[tokio::test]
async fn run_success_with_no_git_progress_and_tool_calls_remains_success() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let (project_root, mut session) = setup_session_repo(&tmp);
    let session_dir =
        csa_session::get_session_dir(&project_root, &session.meta_session_id).expect("dir");

    let executor = build_codex_executor();
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(360);
    let ctx = build_test_ctx(&executor, session_dir, &project_root, start, &hooks_config);
    let mut result = build_test_result("Completed successfully.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(&project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(0));
    assert_eq!(result.exit_code, 0, "tool exit code must remain unchanged");
    let reloaded = load_session(&project_root, &session.meta_session_id).expect("load session");
    assert_ne!(reloaded.termination_reason.as_deref(), Some("no_progress"));
}

#[tokio::test]
async fn run_success_with_success_sidecar_and_no_git_progress_remains_success() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let (project_root, mut session) = setup_session_repo(&tmp);
    let session_dir =
        csa_session::get_session_dir(&project_root, &session.meta_session_id).expect("dir");
    write_result_sidecar(
        &session_dir,
        r#"[result]
status = "success"
summary = "external orchestration passed"
"#,
    );

    let executor = build_codex_executor();
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(15);
    let mut ctx = build_test_ctx(&executor, session_dir, &project_root, start, &hooks_config);
    ctx.has_tool_calls = false;
    let mut result = build_test_result("External orchestration passed.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(&project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(0));
    assert!(
        !persisted
            .summary
            .starts_with("tool exited successfully but produced no changes"),
        "success sidecar must suppress no-progress classification, got: {}",
        persisted.summary
    );
    assert_eq!(result.exit_code, 0);
}

#[tokio::test]
async fn run_success_with_low_evidence_no_git_progress_is_marked_no_progress() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let (project_root, mut session) = setup_session_repo(&tmp);
    let session_dir =
        csa_session::get_session_dir(&project_root, &session.meta_session_id).expect("dir");

    let executor = build_codex_executor();
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(15);
    let mut ctx = build_test_ctx(&executor, session_dir, &project_root, start, &hooks_config);
    ctx.has_tool_calls = false;
    let mut result = build_test_result("Completed successfully.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(&project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, "no_progress");
    assert!(
        persisted
            .summary
            .starts_with("tool exited successfully but produced no changes"),
        "summary should explain no-progress classification, got: {}",
        persisted.summary
    );
    assert_eq!(result.exit_code, 0, "tool exit code must remain unchanged");
    let reloaded = load_session(&project_root, &session.meta_session_id).expect("load session");
    assert_eq!(reloaded.termination_reason.as_deref(), Some("no_progress"));
}

#[tokio::test]
async fn no_progress_detection_does_not_apply_to_review_or_debate() {
    for task_type in ["review", "debate"] {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _sandbox = ScopedSessionSandbox::new(&tmp).await;
        let (project_root, mut session) = setup_session_repo(&tmp);
        let session_dir =
            csa_session::get_session_dir(&project_root, &session.meta_session_id).expect("dir");

        let executor = build_codex_executor();
        let hooks_config = csa_hooks::HooksConfig::default();
        let start = chrono::Utc::now() - chrono::Duration::seconds(360);
        let mut ctx = build_test_ctx(&executor, session_dir, &project_root, start, &hooks_config);
        ctx.task_type = Some(task_type);
        let mut result = build_test_result("Read-only task completed.");

        process_execution_result(ctx, &mut session, &mut result)
            .await
            .expect("process_execution_result");

        let persisted = load_result(&project_root, &session.meta_session_id)
            .expect("load")
            .expect("result exists");
        assert_eq!(persisted.exit_code, 0);
        assert_eq!(
            persisted.status,
            SessionResult::status_from_exit_code(0),
            "status must remain success for task_type={task_type}"
        );
    }
}

#[tokio::test]
async fn run_success_with_uncommitted_changes_remains_success() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let (project_root, mut session) = setup_session_repo(&tmp);
    std::fs::write(project_root.join("tracked.txt"), "changed\n").expect("modify tracked file");
    let session_dir =
        csa_session::get_session_dir(&project_root, &session.meta_session_id).expect("dir");

    let executor = build_codex_executor();
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(360);
    let ctx = build_test_ctx(&executor, session_dir, &project_root, start, &hooks_config);
    let mut result = build_test_result("Applied changes.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(&project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(0));
}

#[tokio::test]
async fn run_success_with_new_commit_remains_success() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let (project_root, mut session) = setup_session_repo(&tmp);
    std::fs::write(project_root.join("tracked.txt"), "committed\n").expect("modify tracked file");
    run_git(&project_root, &["add", "tracked.txt"]);
    run_git(&project_root, &["commit", "-m", "change"]);
    let session_dir =
        csa_session::get_session_dir(&project_root, &session.meta_session_id).expect("dir");

    let executor = build_codex_executor();
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(360);
    let ctx = build_test_ctx(&executor, session_dir, &project_root, start, &hooks_config);
    let mut result = build_test_result("Committed changes.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(&project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(0));
}
