use super::memory_soft_limit_recovery::{
    MEMORY_SOFT_LIMIT_CLEAN_COMMITTED_ACTION, MEMORY_SOFT_LIMIT_CLEAN_COMMITTED_OUTCOME,
    MEMORY_SOFT_LIMIT_DIRTY_ACTION, MEMORY_SOFT_LIMIT_DIRTY_OUTCOME,
    MEMORY_SOFT_LIMIT_NO_WORK_ACTION, MEMORY_SOFT_LIMIT_NO_WORK_OUTCOME,
};
use super::*;
use std::path::Path;
use std::process::Command;

#[test]
fn memory_soft_limit_with_no_changed_paths_records_no_work_recovery() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let mut session_result = session_result("signal", 143);
    session_result.kill_hint = Some("memory_soft_limit".to_string());
    csa_session::save_result(root, &session.meta_session_id, &session_result)
        .expect("result should be saved");
    let mut execution = csa_process::ExecutionResult {
        exit_code: 143,
        summary: "memory soft limit".to_string(),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: false,
            changed_paths: Some(&[]),
            commit_created: Some(false),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    let loaded = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    let recovery = loaded
        .memory_soft_limit_recovery
        .expect("memory-soft-limit no-work recovery should be recorded");
    assert_eq!(recovery.outcome, MEMORY_SOFT_LIMIT_NO_WORK_OUTCOME);
    assert!(!recovery.dirty_worktree);
    assert!(!recovery.commit_created);
    assert!(recovery.changed_paths.is_empty());
    assert_eq!(
        recovery.suggested_recovery_action,
        MEMORY_SOFT_LIMIT_NO_WORK_ACTION
    );
}

#[test]
fn memory_soft_limit_with_dirty_work_records_salvage_recovery() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let mut session_result = session_result("signal", 143);
    session_result.kill_hint = Some("memory_soft_limit".to_string());
    csa_session::save_result(root, &session.meta_session_id, &session_result)
        .expect("result should be saved");
    std::fs::write(root.join("seed.txt"), "seed\npartial\n").expect("dirty file");
    let mut execution = csa_process::ExecutionResult {
        exit_code: 143,
        summary: "memory soft limit".to_string(),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: false,
            changed_paths: Some(&["seed.txt".to_string()]),
            commit_created: Some(false),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    let loaded = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert!(loaded.uncommitted_changes.is_some());
    let recovery = loaded
        .memory_soft_limit_recovery
        .expect("memory-soft-limit dirty recovery should be recorded");
    assert_eq!(recovery.outcome, MEMORY_SOFT_LIMIT_DIRTY_OUTCOME);
    assert!(recovery.dirty_worktree);
    assert!(!recovery.commit_created);
    assert_eq!(recovery.changed_paths, vec!["seed.txt".to_string()]);
    assert_eq!(
        recovery.suggested_recovery_action,
        MEMORY_SOFT_LIMIT_DIRTY_ACTION
    );
}

#[test]
fn memory_soft_limit_with_clean_commit_records_committed_recovery() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let mut session_result = session_result("signal", 143);
    session_result.kill_hint = Some("memory_soft_limit".to_string());
    csa_session::save_result(root, &session.meta_session_id, &session_result)
        .expect("result should be saved");
    std::fs::write(root.join("seed.txt"), "seed\ncommitted\n").expect("edit file");
    run_git(root, &["add", "seed.txt"]);
    run_git(root, &["commit", "-q", "-m", "clean memory recovery"]);
    let mut execution = csa_process::ExecutionResult {
        exit_code: 143,
        summary: "memory soft limit".to_string(),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: false,
            changed_paths: Some(&[]),
            commit_created: Some(true),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    let loaded = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert!(loaded.uncommitted_changes.is_none());
    let recovery = loaded
        .memory_soft_limit_recovery
        .expect("memory-soft-limit clean committed recovery should be recorded");
    assert_eq!(recovery.outcome, MEMORY_SOFT_LIMIT_CLEAN_COMMITTED_OUTCOME);
    assert!(!recovery.dirty_worktree);
    assert!(recovery.commit_created);
    assert!(recovery.changed_paths.is_empty());
    assert!(
        recovery
            .head_oid
            .as_deref()
            .is_some_and(|head| head.len() >= 12)
    );
    assert_eq!(
        recovery.head_summary.as_deref(),
        Some("clean memory recovery")
    );
    assert_eq!(
        recovery.suggested_recovery_action,
        MEMORY_SOFT_LIMIT_CLEAN_COMMITTED_ACTION
    );
}

fn session_result(status: &str, exit_code: i32) -> csa_session::SessionResult {
    let now = chrono::Utc::now();
    csa_session::SessionResult {
        post_exec_gate: None,
        status: status.to_string(),
        exit_code,
        summary: "done".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    }
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
