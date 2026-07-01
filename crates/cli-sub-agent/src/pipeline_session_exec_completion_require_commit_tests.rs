use std::process::Command;

use csa_core::transport_events::StreamingMetadata;
use csa_core::types::OutputFormat;
use csa_executor::{CodexRuntimeMetadata, Executor, TransportResult};
use csa_session::{create_session, get_session_dir, load_result};

use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;

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

fn git_capture(project_root: &std::path::Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn init_git_repo(project_root: &std::path::Path) {
    run_git(project_root, &["init", "-q"]);
    run_git(
        project_root,
        &["config", "user.email", "csa-test@example.com"],
    );
    run_git(project_root, &["config", "user.name", "CSA Test"]);
    run_git(project_root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(project_root.join(".gitignore"), "state/\n").expect("write gitignore");
    std::fs::write(project_root.join("tracked.txt"), "initial\n").expect("write tracked");
    run_git(project_root, &["add", ".gitignore", "tracked.txt"]);
    run_git(project_root, &["commit", "-q", "-m", "initial"]);
}

#[tokio::test]
async fn completion_rescues_require_commit_when_writer_left_uncommitted_changes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    init_git_repo(project_root);
    let initial_head = git_capture(project_root, &["rev-parse", "HEAD"]);
    let before =
        crate::run_cmd::capture_git_workspace_snapshot(project_root, false).expect("snapshot");

    std::fs::write(project_root.join("tracked.txt"), "changed\n").expect("write change");
    run_git(project_root, &["add", "tracked.txt"]);

    let mut session = create_session(
        project_root,
        Some("require commit rescue"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_dir = get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::current(),
    };
    let transport_result = TransportResult {
        execution: csa_process::ExecutionResult {
            output: "writer completed but commit failed".to_string(),
            stderr_output: String::new(),
            summary: "writer completed but commit failed".to_string(),
            exit_code: 0,
            model_completed: Some(true),
            ..Default::default()
        },
        provider_session_id: None,
        events: Vec::new(),
        metadata: StreamingMetadata {
            extracted_commands: vec!["git commit -m fix".to_string()],
            has_tool_calls: true,
            has_execute_tool_calls: true,
            turn_count: 1,
            ..Default::default()
        },
    };
    let plan = SessionCompletionPlan {
        merged_env: Default::default(),
        hooks_config: Default::default(),
        sessions_root: session_dir
            .parent()
            .expect("sessions root")
            .display()
            .to_string(),
        edit_guard: None,
        new_file_guard: None,
        result_file_cleared: false,
        execution_start_time: chrono::Utc::now() - chrono::Duration::seconds(1),
        commit_guard_enabled: true,
        require_commit_on_mutation: true,
        hook_bypass_scan_enabled: true,
        is_git: true,
        inside_git_worktree: true,
        pre_run_workspace: Some(before),
        pre_exec_snapshot: None,
        timeout_diagnostics: None,
        sa_mode: false,
    };

    let completed = complete_session_execution(
        CompletionInput {
            executor: &executor,
            tool: &csa_core::types::ToolName::Codex,
            prompt: "Fix, verify, and commit the work",
            output_format: &OutputFormat::Json,
            task_type: Some("run"),
            readonly_project_root: false,
            project_root,
            config: None,
            global_config: None,
            session_dir: &session_dir,
            memory_project_key: None,
            effective_prompt: "Fix, verify, and commit the work".to_string(),
            plan,
            transport_result,
        },
        &mut session,
    )
    .await
    .expect("complete session");

    assert_eq!(
        completed.execution.exit_code,
        0,
        "summary={}\ngate={:?}\nstderr={}",
        completed.execution.summary,
        completed.execution.csa_gate_failure,
        completed.execution.stderr_output
    );
    assert!(completed.execution.csa_gate_failure.is_none());
    assert_eq!(completed.commit_created, Some(true));
    assert!(
        completed
            .changed_paths
            .as_ref()
            .is_some_and(|paths| paths.len() == 1 && paths[0] == "tracked.txt")
    );
    assert!(
        completed
            .execution
            .stderr_output
            .contains("CSA require-commit rescue: created commit"),
        "{}",
        completed.execution.stderr_output
    );
    assert!(
        !completed
            .execution
            .stderr_output
            .contains("post-run policy blocked"),
        "{}",
        completed.execution.stderr_output
    );
    assert_ne!(
        git_capture(project_root, &["rev-parse", "HEAD"]),
        initial_head
    );
    assert_eq!(git_capture(project_root, &["status", "--porcelain=v1"]), "");
    assert_eq!(
        git_capture(project_root, &["log", "-1", "--format=%s"]),
        "feat: auto-rescue commit from CSA codex writer session"
    );
    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load result")
        .expect("result should be saved");
    assert_eq!(persisted.status, "success");
    assert_eq!(persisted.exit_code, 0);
}

#[test]
fn require_commit_rescue_is_not_attempted_when_head_already_changed() {
    let guard = crate::run_cmd::PostRunCommitGuard {
        workspace_mutated: true,
        head_changed: true,
        head_externally_raced: false,
        changed_paths: vec!["tracked.txt".to_string()],
    };

    assert!(!super::require_commit::should_attempt_require_commit_rescue(true, Some(&guard)));
}
