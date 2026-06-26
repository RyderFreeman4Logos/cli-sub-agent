use std::path::Path;
use std::process::Command;

use super::*;

#[test]
fn require_commit_with_commit_created_ignores_untracked_scratch() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let mut session_result = session_result("success", 0);
    csa_session::save_result(root, &session.meta_session_id, &session_result)
        .expect("result should be saved");
    std::fs::write(root.join("src.rs"), "dirty\n").expect("dirty file");
    let mut execution = csa_process::ExecutionResult {
        exit_code: 0,
        summary: "done".to_string(),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: true,
            changed_paths: Some(&["src.rs".to_string()]),
            commit_created: Some(true),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    session_result = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert_eq!(execution.exit_code, 0);
    assert_eq!(session_result.status, "success");
    assert!(session_result.require_commit_recovery.is_none());
}

#[test]
fn require_commit_with_commit_created_and_dirty_tracked_work_fails() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    std::fs::write(root.join("tracked.txt"), "committed\n").expect("write committed change");
    run_git(root, &["add", "tracked.txt"]);
    run_git(root, &["commit", "-q", "-m", "advance tracked"]);
    std::fs::write(root.join("tracked.txt"), "dirty after commit\n").expect("dirty tracked file");

    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let mut session_result = session_result("success", 0);
    session_result.summary = "self-reported success".to_string();
    csa_session::save_result(root, &session.meta_session_id, &session_result)
        .expect("result should be saved");
    let mut execution = csa_process::ExecutionResult {
        exit_code: 0,
        summary: "self-reported success".to_string(),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: true,
            changed_paths: Some(&["tracked.txt".to_string()]),
            commit_created: Some(true),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    let loaded = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert_eq!(execution.exit_code, 1);
    assert_eq!(
        execution.csa_gate_failure.as_deref(),
        Some("writer-uncommitted")
    );
    assert_eq!(loaded.status, "failure");
    assert_eq!(loaded.exit_code, 1);
    assert_eq!(loaded.summary, REQUIRE_COMMIT_REASON);
    let recovery = loaded
        .require_commit_recovery
        .expect("dirty tracked work should be a require-commit contract failure");
    assert!(recovery.require_commit);
    assert!(recovery.commit_created);
    assert!(recovery.dirty_worktree);
    assert_eq!(recovery.changed_paths, vec!["tracked.txt".to_string()]);
    assert_eq!(
        recovery.blocker_summary.as_deref(),
        Some("summary=self-reported success")
    );
}

#[test]
fn require_commit_with_commit_created_and_clean_tracked_work_passes() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let mut session_result = session_result("success", 0);
    csa_session::save_result(root, &session.meta_session_id, &session_result)
        .expect("result should be saved");
    std::fs::write(root.join("tracked.txt"), "committed\n").expect("write committed change");
    run_git(root, &["add", "tracked.txt"]);
    run_git(root, &["commit", "-q", "-m", "clean tracked commit"]);
    let mut execution = csa_process::ExecutionResult {
        exit_code: 0,
        summary: "done".to_string(),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: true,
            changed_paths: Some(&[]),
            commit_created: Some(true),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    session_result = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert_eq!(execution.exit_code, 0);
    assert_eq!(session_result.status, "success");
    assert!(session_result.uncommitted_changes.is_none());
    assert!(session_result.require_commit_recovery.is_none());
}

#[test]
fn require_commit_with_commit_created_fails_when_clean_tree_probe_is_unknown() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let mut session_result = session_result("success", 0);
    session_result.summary = "self-reported success".to_string();
    csa_session::save_result(root, &session.meta_session_id, &session_result)
        .expect("result should be saved");
    let hidden_git_dir = root.join(".git-unavailable-for-status");
    std::fs::rename(root.join(".git"), &hidden_git_dir).expect("git metadata should move aside");
    let mut execution = csa_process::ExecutionResult {
        exit_code: 0,
        summary: "self-reported success".to_string(),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: true,
            changed_paths: Some(&[]),
            commit_created: Some(true),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );
    std::fs::rename(hidden_git_dir, root.join(".git")).expect("git metadata should be restored");

    let loaded = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert_eq!(execution.exit_code, 1);
    assert_eq!(
        execution.csa_gate_failure.as_deref(),
        Some("writer-uncommitted")
    );
    assert_eq!(loaded.status, "failure");
    let recovery = loaded
        .require_commit_recovery
        .expect("unverified clean tree should be a require-commit contract failure");
    assert!(recovery.commit_created);
    assert!(!recovery.dirty_worktree);
    assert!(recovery.changed_paths.is_empty());
    let blocker = recovery
        .blocker_summary
        .expect("clean-tree verification failure should be reported");
    assert!(blocker.contains("clean_tree_verification=git-status-probe-failed"));
    assert!(blocker.contains("summary=self-reported success"));
    assert!(!blocker.contains(root.to_string_lossy().as_ref()));
}

#[test]
fn require_commit_without_created_commit_fails_successful_self_report() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let session_result = session_result("success", 0);
    csa_session::save_result(root, &session.meta_session_id, &session_result)
        .expect("result should be saved");
    std::fs::write(root.join("preexisting.txt"), "dirty\n").expect("dirty file");
    let mut execution = csa_process::ExecutionResult {
        exit_code: 0,
        summary: "done".to_string(),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: true,
            changed_paths: Some(&[]),
            commit_created: Some(false),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    let loaded = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert_eq!(execution.exit_code, 1);
    assert_eq!(loaded.status, "failure");
    assert!(loaded.uncommitted_changes.is_none());
    let recovery = loaded
        .require_commit_recovery
        .expect("require-commit failure should be machine-readable");
    assert!(recovery.require_commit);
    assert!(!recovery.commit_created);
    assert!(!recovery.dirty_worktree);
    assert!(recovery.changed_paths.is_empty());
    assert_eq!(recovery.blocker_summary.as_deref(), Some("summary=done"));
}

#[test]
fn require_commit_applies_in_sa_mode_when_hook_leaves_staged_work() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    std::fs::write(root.join("tracked.txt"), "staged but not committed\n")
        .expect("write tracked change");
    run_git(root, &["add", "tracked.txt"]);
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let session_result = session_result("success", 0);
    csa_session::save_result(root, &session.meta_session_id, &session_result)
        .expect("result should be saved");
    let mut execution = csa_process::ExecutionResult {
        exit_code: 0,
        summary: "hook failed but employee self-reported success".to_string(),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: true,
            require_commit: true,
            changed_paths: Some(&["tracked.txt".to_string()]),
            commit_created: Some(false),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    let loaded = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert_eq!(execution.exit_code, 1);
    assert_eq!(
        execution.csa_gate_failure.as_deref(),
        Some("writer-uncommitted")
    );
    assert_eq!(loaded.status, "failure");
    assert_eq!(loaded.exit_code, 1);
    assert_eq!(loaded.summary, REQUIRE_COMMIT_REASON);
    let recovery = loaded
        .require_commit_recovery
        .expect("explicit require-commit must fail closed in sa-mode");
    assert!(recovery.require_commit);
    assert!(!recovery.commit_created);
    assert!(recovery.dirty_worktree);
    assert_eq!(recovery.changed_paths, vec!["tracked.txt".to_string()]);
    assert_eq!(
        recovery.suggested_recovery_action,
        REQUIRE_COMMIT_RECOVERY_ACTION
    );
}

#[test]
fn require_commit_recovery_records_bounded_redacted_blocker_summary() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    std::fs::write(root.join("seed.txt"), "dirty\n").expect("dirty tracked file");
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let session_result = session_result("failure", 2);
    let mut session_result = csa_session::SessionResult {
        summary: format!(
            "rustup toolchain setup failed before commit; api_key={} {}",
            "***",
            "x".repeat(400)
        ),
        ..session_result
    };
    session_result.raw_process_exit_code = Some(2);
    csa_session::save_result(root, &session.meta_session_id, &session_result)
        .expect("result should be saved");
    let mut execution = csa_process::ExecutionResult {
        exit_code: 2,
        summary: session_result.summary.clone(),
        csa_gate_failure: Some("commit-policy-uncommitted".to_string()),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: true,
            changed_paths: Some(&["seed.txt".to_string()]),
            commit_created: Some(false),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    let loaded = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    let recovery = loaded
        .require_commit_recovery
        .expect("require-commit failure should include recovery diagnostic");
    let blocker = recovery
        .blocker_summary
        .expect("blocker summary should be recorded");
    assert!(blocker.contains("gate=commit-policy-uncommitted"));
    assert!(blocker.contains("rustup toolchain setup failed before commit"));
    assert!(!blocker.contains("***"));
    assert!(blocker.contains("[REDACTED]"));
    assert!(blocker.chars().count() <= REQUIRE_COMMIT_BLOCKER_SUMMARY_MAX_CHARS);
    assert!(recovery.dirty_worktree);
    assert_eq!(recovery.changed_paths, vec!["seed.txt".to_string()]);
}

#[test]
fn require_commit_recovery_does_not_label_untracked_scratch_as_tracked_dirty() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    std::fs::write(root.join("scratch.txt"), "untracked\n").expect("untracked scratch file");
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let session_result = session_result("success", 0);
    csa_session::save_result(root, &session.meta_session_id, &session_result)
        .expect("result should be saved");
    let mut execution = csa_process::ExecutionResult {
        exit_code: 0,
        summary: "self-reported success".to_string(),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: true,
            changed_paths: Some(&["scratch.txt".to_string()]),
            commit_created: Some(false),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    let loaded = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert_eq!(execution.exit_code, 1);
    assert_eq!(loaded.status, "failure");
    let recovery = loaded
        .require_commit_recovery
        .expect("missing commit should be a require-commit contract failure");
    assert!(!recovery.commit_created);
    assert!(!recovery.dirty_worktree);
    assert!(recovery.changed_paths.is_empty());
    let uncommitted = loaded
        .uncommitted_changes
        .expect("untracked session scratch should still be reported as uncommitted work");
    assert_eq!(uncommitted.files, vec!["scratch.txt".to_string()]);
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

/// A throwaway git repo with one commit so `HEAD` exists. Hooks and GPG
/// signing are disabled so the test stays hermetic regardless of the host's
/// global git config.
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
