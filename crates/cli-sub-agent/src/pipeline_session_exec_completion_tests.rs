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
    String::from_utf8(output.stdout)
        .expect("git output should be utf8")
        .trim()
        .to_string()
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
async fn completion_warns_when_commit_reflog_is_followed_by_external_checkout() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    init_git_repo(project_root);
    let primary_branch = git_capture(project_root, &["branch", "--show-current"]);

    run_git(project_root, &["checkout", "-q", "-b", "other"]);
    std::fs::write(project_root.join("tracked.txt"), "other\n").expect("write other");
    run_git(project_root, &["commit", "-am", "other"]);
    run_git(project_root, &["checkout", "-q", &primary_branch]);

    let before =
        crate::run_cmd::capture_git_workspace_snapshot(project_root, false).expect("snapshot");
    let execution_start_time = chrono::Utc::now() - chrono::Duration::seconds(1);
    std::fs::write(project_root.join("tracked.txt"), "child\n").expect("write child");
    run_git(project_root, &["commit", "-am", "child commit"]);
    run_git(project_root, &["checkout", "-q", "other"]);

    let mut session = create_session(
        project_root,
        Some("external checkout after commit"),
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
            output: String::new(),
            stderr_output: "nothing to commit, working tree clean".to_string(),
            summary: "nothing to commit".to_string(),
            exit_code: 1,
            model_completed: Some(true),
            ..Default::default()
        },
        provider_session_id: None,
        events: Vec::new(),
        metadata: StreamingMetadata {
            extracted_commands: vec!["git commit -m 'child commit'".to_string()],
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
        execution_start_time,
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
            prompt: "Commit the work",
            output_format: &OutputFormat::Json,
            task_type: Some("run"),
            readonly_project_root: false,
            project_root,
            config: None,
            global_config: None,
            session_dir: &session_dir,
            memory_project_key: None,
            effective_prompt: "Commit the work".to_string(),
            plan,
            transport_result,
        },
        &mut session,
    )
    .await
    .expect("complete session");

    assert_eq!(completed.commit_created, Some(true));
    assert!(completed.execution.csa_gate_failure.is_none());
    assert!(
        completed.execution.stderr_output.contains(
            "external checkout/reset moved the worktree before session completion (#2570)"
        ),
        "{}",
        completed.execution.stderr_output
    );
}

#[tokio::test]
async fn completion_fails_when_git_commit_attempt_leaves_head_unchanged_and_staged() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    init_git_repo(project_root);
    let before =
        crate::run_cmd::capture_git_workspace_snapshot(project_root, false).expect("snapshot");

    std::fs::write(project_root.join("tracked.txt"), "changed\n").expect("write change");
    run_git(project_root, &["add", "tracked.txt"]);

    let mut session = create_session(project_root, Some("commit ref update"), None, Some("codex"))
        .expect("create session");
    let session_dir = get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::current(),
    };
    let transport_result = TransportResult {
        execution: csa_process::ExecutionResult {
            output: "git commit reported an object id before ref update failed".to_string(),
            stderr_output: String::new(),
            summary: "created commit object 49a0bad".to_string(),
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
        require_commit_on_mutation: false,
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

    assert_eq!(completed.execution.exit_code, 1);
    assert_eq!(
        completed.execution.csa_gate_failure.as_deref(),
        Some("commit-policy-ref-update")
    );
    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load result")
        .expect("result should be saved");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 1);
}

#[tokio::test]
async fn completion_reports_commit_created_when_head_advances_cleanly() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    init_git_repo(project_root);
    let before =
        crate::run_cmd::capture_git_workspace_snapshot(project_root, false).expect("snapshot");

    std::fs::write(project_root.join("tracked.txt"), "committed\n").expect("write change");
    run_git(project_root, &["add", "tracked.txt"]);
    run_git(project_root, &["commit", "-q", "-m", "clean commit"]);

    let mut session = create_session(project_root, Some("clean commit"), None, Some("codex"))
        .expect("create session");
    let session_dir = get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::current(),
    };
    let transport_result = TransportResult {
        execution: csa_process::ExecutionResult {
            output: "committed".to_string(),
            stderr_output: String::new(),
            summary: "committed".to_string(),
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
            prompt: "Commit the work",
            output_format: &OutputFormat::Json,
            task_type: Some("run"),
            readonly_project_root: false,
            project_root,
            config: None,
            global_config: None,
            session_dir: &session_dir,
            memory_project_key: None,
            effective_prompt: "Commit the work".to_string(),
            plan,
            transport_result,
        },
        &mut session,
    )
    .await
    .expect("complete session");

    assert_eq!(completed.commit_created, Some(true));
    assert_eq!(completed.execution.exit_code, 0);
    assert!(
        !completed
            .execution
            .stderr_output
            .contains("CSA require-commit rescue"),
        "{}",
        completed.execution.stderr_output
    );
}

#[test]
fn fix_finding_terminal_guard_allows_dirty_side_effects_after_amend() {
    let mut session = MetaSessionState::default();
    session.task_context.task_type = Some(REVIEW_FIX_FINDING_TASK_TYPE.to_string());
    let commit_guard = crate::run_cmd::PostRunCommitGuard {
        workspace_mutated: true,
        head_changed: true,
        head_externally_raced: false,
        changed_paths: vec!["tracked.txt".to_string()],
    };
    let mut result = csa_process::ExecutionResult {
        exit_code: 0,
        model_completed: Some(true),
        ..Default::default()
    };

    apply_fix_finding_terminal_guard(&session, Some(true), Some(&commit_guard), &mut result);

    assert_eq!(result.exit_code, 0);
    assert!(result.csa_gate_failure.is_none());
}
