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

fn signal_transport_result(exit_code: i32, terminal_reason: &str) -> TransportResult {
    TransportResult {
        execution: csa_process::ExecutionResult {
            output: String::new(),
            stderr_output: format!("Execution interrupted by {terminal_reason}"),
            summary: format!("Execution interrupted by {terminal_reason}"),
            exit_code,
            model_completed: Some(false),
            terminal_reason: Some(terminal_reason.to_string()),
            raw_process_exit_code: Some(exit_code),
            exit_signal: (terminal_reason == "sigterm").then_some(libc::SIGTERM),
            ..Default::default()
        },
        provider_session_id: None,
        events: Vec::new(),
        metadata: StreamingMetadata {
            turn_count: 1,
            ..Default::default()
        },
    }
}

fn register_memory_soft_limit_evidence(session_dir: &std::path::Path) {
    let diagnostic_path =
        csa_resource::memory_monitor::soft_limit_diagnostic_path_for_session_dir(session_dir)
            .expect("memory soft-limit diagnostic path");
    let event = csa_resource::memory_monitor::MemorySoftLimitKillDiagnostic {
        kill_hint: csa_resource::memory_monitor::MEMORY_SOFT_LIMIT_KILL_HINT.to_string(),
        signal: libc::SIGTERM,
        current_mb: 9216,
        threshold_mb: 8601,
        memory_max_mb: 12_288,
        soft_limit_percent: 70,
        scope_name: "csa-codex-01KTEST.scope".to_string(),
    };
    csa_resource::memory_monitor::record_soft_limit_diagnostic_evidence(&diagnostic_path, &event);
}

#[tokio::test]
async fn signal_killed_run_records_dirty_workspace_and_memory_recovery() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    init_git_repo(project_root);
    let before =
        crate::run_cmd::capture_git_workspace_snapshot(project_root, false).expect("snapshot");

    let mut session = create_session(
        project_root,
        Some("signal killed dirty workspace"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id.clone();
    let session_dir = get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    register_memory_soft_limit_evidence(&session_dir);
    std::fs::write(project_root.join("tracked.txt"), "dirty\n").expect("write change");

    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::current(),
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

    let mut completed = complete_session_execution(
        CompletionInput {
            executor: &executor,
            tool: &csa_core::types::ToolName::Codex,
            prompt: "Fix the work",
            output_format: &OutputFormat::Json,
            task_type: Some("run"),
            readonly_project_root: false,
            project_root,
            config: None,
            global_config: None,
            session_dir: &session_dir,
            memory_project_key: None,
            effective_prompt: "Fix the work".to_string(),
            plan,
            transport_result: signal_transport_result(143, "sigterm"),
        },
        &mut session,
    )
    .await
    .expect("complete session");

    crate::run_cmd::record_run_dirty(
        project_root,
        Some(&session_id),
        &mut completed.execution,
        completed.changed_paths.as_deref(),
        completed.commit_created,
        false,
        None,
    );

    let persisted = load_result(project_root, &session_id)
        .expect("load result")
        .expect("result should be saved");
    assert_eq!(persisted.status, "interrupted");
    assert_eq!(persisted.exit_code, 143);
    assert_eq!(persisted.kill_hint.as_deref(), Some("memory_soft_limit"));
    let changes = persisted
        .uncommitted_changes
        .as_ref()
        .expect("dirty workspace should be recorded");
    assert!(changes.files.iter().any(|path| path == "tracked.txt"));
    let recovery = persisted
        .memory_soft_limit_recovery
        .as_ref()
        .expect("memory soft-limit recovery should be recorded");
    assert_eq!(recovery.outcome, "dirty_or_staged_changes");
    assert!(
        recovery
            .changed_paths
            .iter()
            .any(|path| path == "tracked.txt")
    );
    assert!(
        recovery
            .git_status_short
            .iter()
            .any(|line| line.contains("tracked.txt")),
        "{:?}",
        recovery.git_status_short
    );
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn signal_killed_require_commit_run_rescues_dirty_workspace() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    init_git_repo(project_root);
    let initial_head = git_capture(project_root, &["rev-parse", "HEAD"]);
    let before =
        crate::run_cmd::capture_git_workspace_snapshot(project_root, false).expect("snapshot");

    std::fs::write(project_root.join("tracked.txt"), "dirty\n").expect("write change");

    let mut session = create_session(
        project_root,
        Some("signal killed require commit"),
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
            transport_result: signal_transport_result(143, "sigterm"),
        },
        &mut session,
    )
    .await
    .expect("complete session");

    let new_head = git_capture(project_root, &["rev-parse", "HEAD"]);
    assert_ne!(new_head, initial_head);
    assert_eq!(completed.commit_created, Some(true));
    assert!(
        completed
            .execution
            .stderr_output
            .contains("CSA require-commit rescue: created commit"),
        "{}",
        completed.execution.stderr_output
    );
    assert_eq!(git_capture(project_root, &["status", "--short"]), "");
}

#[cfg(not(target_os = "macos"))]
async fn assert_completion_does_not_rescue_after_sandbox_hook_failure_marker(
    hang_git_probe: bool,
    sa_mode: bool,
) {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let mut sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    init_git_repo(project_root);
    let initial_head = git_capture(project_root, &["rev-parse", "HEAD"]);
    let before =
        crate::run_cmd::capture_git_workspace_snapshot(project_root, false).expect("snapshot");

    std::fs::write(project_root.join("tracked.txt"), "changed\n").expect("write change");
    run_git(project_root, &["add", "tracked.txt"]);
    let staged_tree = git_capture(project_root, &["write-tree"]);

    let mut session = create_session(
        project_root,
        Some("sandbox hook blocked require commit"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_dir = get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    std::fs::write(
        session_dir.join(csa_hooks::git_guard::SANDBOX_COMMIT_FAILURE_MARKER_FILE),
        format!("{initial_head} {staged_tree} args env config hooks\n"),
    )
    .expect("write sandbox hook marker");

    let original_path = hang_git_probe.then(|| std::env::var_os("PATH").unwrap_or_default());
    if let Some(original_path) = original_path.as_ref() {
        let fake_bin = project_root.join("fake-bin");
        std::fs::create_dir(&fake_bin).expect("create fake bin");
        let fake_git = fake_bin.join("git");
        std::fs::write(
            &fake_git,
            r#"#!/bin/sh
if [ "${1:-}" = "-C" ] && [ "${3:-}" = "rev-parse" ] && [ "${4:-}" = "--verify" ] && [ "${5:-}" = "HEAD" ]; then
  count=0
  [ ! -f "${GIT_PROBE_COUNT}" ] || count="$(cat "${GIT_PROBE_COUNT}")"
  count=$((count + 1))
  printf '%s\n' "${count}" > "${GIT_PROBE_COUNT}"
  [ "${count}" -ne 2 ] || exec /usr/bin/sleep 2
fi
exec /usr/bin/git "$@"
"#,
        )
        .expect("write hanging probe Git");
        std::fs::set_permissions(&fake_git, std::fs::Permissions::from_mode(0o755))
            .expect("make fake Git executable");
        let probe_count = project_root.join("git-probe-count");
        let mut path = fake_bin.into_os_string();
        path.push(":");
        path.push(original_path);
        sandbox.track_env("PATH");
        sandbox.track_env("GIT_PROBE_COUNT");
        // SAFETY: ScopedSessionSandbox owns TEST_ENV_LOCK and restores both variables.
        unsafe {
            std::env::set_var("PATH", path);
            std::env::set_var("GIT_PROBE_COUNT", probe_count);
        }
    }
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
        sa_mode,
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

    if let Some(original_path) = original_path {
        // SAFETY: restore PATH before the fixture's final real-Git checks.
        unsafe {
            std::env::set_var("PATH", original_path);
            std::env::remove_var("GIT_PROBE_COUNT");
        }
    }

    assert_eq!(completed.commit_created, Some(false));
    if sa_mode {
        assert!(completed.execution.summary.contains(
            "require-commit blocked: mandatory hook-enabled commit failed in the filesystem sandbox"
        ));
    }
    assert_ne!(completed.execution.exit_code, 0);
    assert_eq!(
        git_capture(project_root, &["rev-parse", "HEAD"]),
        initial_head
    );
    assert_eq!(
        git_capture(project_root, &["write-tree"]),
        staged_tree,
        "control-plane rescue must preserve the sandbox-blocked staged tree"
    );
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn completion_does_not_rescue_after_sandbox_hook_failure_marker() {
    assert_completion_does_not_rescue_after_sandbox_hook_failure_marker(false, false).await;
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn dirty_sa_preserves_sandbox_hook_failure_marker() {
    assert_completion_does_not_rescue_after_sandbox_hook_failure_marker(false, true).await;
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn completion_does_not_rescue_when_sandbox_marker_probe_times_out() {
    assert_completion_does_not_rescue_after_sandbox_hook_failure_marker(true, false).await;
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

#[tokio::test]
async fn sigint_killed_require_commit_run_does_not_rescue_dirty_workspace() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    init_git_repo(project_root);
    let initial_head = git_capture(project_root, &["rev-parse", "HEAD"]);
    let before =
        crate::run_cmd::capture_git_workspace_snapshot(project_root, false).expect("snapshot");

    std::fs::write(project_root.join("tracked.txt"), "dirty\n").expect("write change");

    let mut session = create_session(
        project_root,
        Some("sigint cancelled require commit"),
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

    let mut sigint_result = signal_transport_result(130, "sigint");
    sigint_result.execution.exit_signal = Some(libc::SIGINT);

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
            transport_result: sigint_result,
        },
        &mut session,
    )
    .await
    .expect("complete session");

    let new_head = git_capture(project_root, &["rev-parse", "HEAD"]);
    assert_eq!(
        new_head, initial_head,
        "SIGINT should not trigger rescue commit"
    );
    assert!(
        !completed
            .execution
            .stderr_output
            .contains("CSA require-commit rescue"),
        "no rescue commit expected for SIGINT"
    );
}

#[tokio::test]
async fn timeout_killed_require_commit_run_does_not_rescue_dirty_workspace() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    init_git_repo(project_root);
    let initial_head = git_capture(project_root, &["rev-parse", "HEAD"]);
    let before =
        crate::run_cmd::capture_git_workspace_snapshot(project_root, false).expect("snapshot");

    std::fs::write(project_root.join("tracked.txt"), "dirty\n").expect("write change");

    let mut session = create_session(
        project_root,
        Some("timeout cancelled require commit"),
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
            transport_result: signal_transport_result(124, "timeout"),
        },
        &mut session,
    )
    .await
    .expect("complete session");

    let new_head = git_capture(project_root, &["rev-parse", "HEAD"]);
    assert_eq!(
        new_head, initial_head,
        "timeout should not trigger rescue commit"
    );
    assert!(
        !completed
            .execution
            .stderr_output
            .contains("CSA require-commit rescue"),
        "no rescue commit expected for timeout"
    );
}
