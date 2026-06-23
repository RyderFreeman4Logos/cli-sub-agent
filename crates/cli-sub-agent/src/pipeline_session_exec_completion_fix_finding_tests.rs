use std::process::Command;

use csa_core::transport_events::StreamingMetadata;
use csa_core::types::OutputFormat;
use csa_executor::{CodexRuntimeMetadata, TransportResult};
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

fn init_git_repo(project_root: &std::path::Path) {
    run_git(project_root, &["init", "-q"]);
    run_git(
        project_root,
        &["config", "user.email", "csa-test@example.com"],
    );
    run_git(project_root, &["config", "user.name", "CSA Test"]);
    run_git(project_root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(project_root.join("tracked.txt"), "initial\n").expect("write tracked");
    run_git(project_root, &["add", "tracked.txt"]);
    run_git(project_root, &["commit", "-q", "-m", "initial"]);
}

#[tokio::test]
async fn completion_fails_fix_finding_reviewer_sub_session_when_dirty_without_amend() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    init_git_repo(project_root);
    let mut session = create_session(project_root, Some("fix finding"), None, Some("codex"))
        .expect("create session");
    session.task_context.task_type = Some(REVIEW_FIX_FINDING_TASK_TYPE.to_string());
    let session_dir = get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    let before =
        crate::run_cmd::capture_git_workspace_snapshot(project_root, true).expect("snapshot");
    session.git_head_at_creation = before.head.clone();
    session.pre_session_porcelain = Some(before.status.clone());

    std::fs::write(project_root.join("tracked.txt"), "fixed but not amended\n")
        .expect("write dirty fix");

    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::current(),
    };
    let transport_result = TransportResult {
        execution: csa_process::ExecutionResult {
            output: "applied fix but did not amend".to_string(),
            stderr_output: String::new(),
            summary: "applied fix".to_string(),
            exit_code: 0,
            model_completed: Some(true),
            ..Default::default()
        },
        provider_session_id: None,
        events: Vec::new(),
        metadata: StreamingMetadata {
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
            prompt: "Fix the confirmed review finding and amend the commit",
            output_format: &OutputFormat::Json,
            task_type: Some("reviewer_sub_session"),
            readonly_project_root: false,
            project_root,
            config: None,
            global_config: None,
            session_dir: &session_dir,
            memory_project_key: None,
            effective_prompt: "Fix the confirmed review finding and amend the commit".to_string(),
            plan,
            transport_result,
        },
        &mut session,
    )
    .await
    .expect("complete session");

    assert_eq!(completed.execution.exit_code, 1);
    assert_eq!(
        completed.execution.csa_gate_failure.as_deref(),
        Some("commit-policy-uncommitted")
    );
    assert!(completed.execution.summary.contains("--fix-finding"));
    assert!(
        completed
            .execution
            .summary
            .contains("no qualifying amend/commit")
    );
    assert!(
        completed
            .execution
            .summary
            .contains("modified=[tracked.txt]")
    );
    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load result")
        .expect("result should be saved");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 1);
    assert!(persisted.summary.contains("--fix-finding"));
    assert!(persisted.summary.contains("git status --short"));
}

#[tokio::test]
async fn completion_fails_fix_finding_reviewer_sub_session_when_no_amend_completed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    init_git_repo(project_root);
    let mut session = create_session(project_root, Some("fix finding"), None, Some("codex"))
        .expect("create session");
    session.task_context.task_type = Some(REVIEW_FIX_FINDING_TASK_TYPE.to_string());
    let session_dir = get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    let before =
        crate::run_cmd::capture_git_workspace_snapshot(project_root, true).expect("snapshot");
    session.git_head_at_creation = before.head.clone();
    session.pre_session_porcelain = Some(before.status.clone());
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::current(),
    };
    let transport_result = TransportResult {
        execution: csa_process::ExecutionResult {
            output: "reported fix complete without amend".to_string(),
            stderr_output: String::new(),
            summary: "fix complete".to_string(),
            exit_code: 0,
            model_completed: Some(true),
            ..Default::default()
        },
        provider_session_id: None,
        events: Vec::new(),
        metadata: StreamingMetadata {
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
            prompt: "Fix the confirmed review finding and amend the commit",
            output_format: &OutputFormat::Json,
            task_type: Some("reviewer_sub_session"),
            readonly_project_root: false,
            project_root,
            config: None,
            global_config: None,
            session_dir: &session_dir,
            memory_project_key: None,
            effective_prompt: "Fix the confirmed review finding and amend the commit".to_string(),
            plan,
            transport_result,
        },
        &mut session,
    )
    .await
    .expect("complete session");

    assert_eq!(completed.execution.exit_code, 1);
    assert_eq!(
        completed.execution.csa_gate_failure.as_deref(),
        Some("commit-policy-ref-update")
    );
    assert!(
        completed
            .execution
            .summary
            .contains("no qualifying amend/commit")
    );
    assert!(
        completed
            .execution
            .summary
            .contains("repo_side_effects=none_detected")
    );
}
