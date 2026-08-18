#[cfg(not(target_os = "macos"))]
use std::process::Command;

#[cfg(not(target_os = "macos"))]
use csa_core::transport_events::StreamingMetadata;
#[cfg(not(target_os = "macos"))]
use csa_core::types::OutputFormat;
#[cfg(not(target_os = "macos"))]
use csa_executor::{CodexRuntimeMetadata, Executor, TransportResult};
#[cfg(not(target_os = "macos"))]
use csa_session::{create_session, get_session_dir};

#[cfg(not(target_os = "macos"))]
use super::*;
#[cfg(not(target_os = "macos"))]
use crate::test_session_sandbox::ScopedSessionSandbox;

#[cfg(not(target_os = "macos"))]
fn git(project_root: &std::path::Path, args: &[&str]) -> std::process::Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .expect("run git");
    assert!(output.status.success(), "git {} failed", args.join(" "));
    output
}

#[cfg(not(target_os = "macos"))]
fn git_text(project_root: &std::path::Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git(project_root, args).stdout)
        .trim()
        .to_string()
}

#[cfg(not(target_os = "macos"))]
fn init_git_repo(project_root: &std::path::Path) {
    git(project_root, &["init", "-q"]);
    git(
        project_root,
        &["config", "user.email", "csa-test@example.com"],
    );
    git(project_root, &["config", "user.name", "CSA Test"]);
    git(project_root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(project_root.join("tracked.txt"), "initial\n").expect("write tracked");
    git(project_root, &["add", "tracked.txt"]);
    git(project_root, &["commit", "-q", "-m", "initial"]);
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn completion_does_not_rescue_after_autofix_hook_failure() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path();
    init_git_repo(project_root);
    let initial_head = git_text(project_root, &["rev-parse", "HEAD"]);
    let before =
        crate::run_cmd::capture_git_workspace_snapshot(project_root, false).expect("snapshot");
    std::fs::write(project_root.join("tracked.txt"), "changed\n").expect("write change");
    git(project_root, &["add", "tracked.txt"]);

    let mut session = create_session(
        project_root,
        Some("autofix hook failure"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_dir = get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    let wrapper = session_dir.join("bin/git");
    std::fs::create_dir_all(wrapper.parent().expect("wrapper parent")).expect("create wrapper dir");
    std::fs::write(&wrapper, csa_hooks::git_wrapper_script()).expect("write git wrapper");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))
        .expect("make wrapper executable");
    let hook = project_root.join(".git/hooks/pre-commit");
    let hook_generated = project_root.join("hook-generated.txt");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\nprintf 'autofixed\\n' > {}\n/usr/bin/git add {}\nexit 1\n",
            hook_generated.display(),
            hook_generated.display(),
        ),
    )
    .expect("write autofix hook");
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
        .expect("make hook executable");
    assert!(
        !Command::new(&wrapper)
            .args(["commit", "-m", "sandbox commit"])
            .current_dir(project_root)
            .env("CSA_REAL_GIT", "/usr/bin/git")
            .env("CSA_FS_SANDBOXED", "1")
            .env("CSA_SESSION_DIR", &session_dir)
            .output()
            .expect("run autofix hook")
            .status
            .success()
    );
    assert!(
        session_dir
            .join(csa_hooks::git_guard::SANDBOX_COMMIT_FAILURE_MARKER_FILE)
            .is_file()
    );
    let staged_tree = git_text(project_root, &["write-tree"]);

    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::current(),
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
            plan: SessionCompletionPlan {
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
            },
            transport_result: TransportResult {
                execution: csa_process::ExecutionResult {
                    output: "writer completed but commit failed".to_string(),
                    summary: "writer completed but commit failed".to_string(),
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
            },
        },
        &mut session,
    )
    .await
    .expect("complete session");

    assert_eq!(completed.commit_created, Some(false));
    assert_ne!(completed.execution.exit_code, 0);
    assert_eq!(git_text(project_root, &["rev-parse", "HEAD"]), initial_head);
    assert_eq!(git_text(project_root, &["write-tree"]), staged_tree);
}
