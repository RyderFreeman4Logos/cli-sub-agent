use crate::session_guard::{SessionCleanupGuard, write_pre_exec_error_result};
use crate::test_session_sandbox::ScopedSessionSandbox;
use std::fs;

/// Verify that `SessionCleanupGuard` removes the directory on drop when not defused.
#[test]
fn cleanup_guard_removes_orphan_dir_on_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let orphan_dir = tmp.path().join("orphan-session");
    fs::create_dir_all(&orphan_dir).unwrap();
    assert!(orphan_dir.exists());

    {
        let _guard = SessionCleanupGuard::new(orphan_dir.clone());
        // guard drops here without defuse
    }

    assert!(
        !orphan_dir.exists(),
        "cleanup guard must remove orphan session directory on drop"
    );
}

/// Verify that `SessionCleanupGuard` preserves the directory when defused.
#[test]
fn cleanup_guard_preserves_dir_when_defused() {
    let tmp = tempfile::tempdir().unwrap();
    let session_dir = tmp.path().join("good-session");
    fs::create_dir_all(&session_dir).unwrap();
    assert!(session_dir.exists());

    {
        let mut guard = SessionCleanupGuard::new(session_dir.clone());
        guard.defuse();
        // guard drops here after defuse
    }

    assert!(
        session_dir.exists(),
        "cleanup guard must preserve session directory when defused"
    );
}

/// Verify that pre-execution failures preserve the session directory (defuse + result.toml).
///
/// This tests the pattern used in `execute_with_session_and_meta`: when a
/// pre-execution step fails, we write an error `result.toml` and defuse the
/// guard so the session directory survives with a failure record instead of
/// being deleted as an orphan.
#[test]
fn pre_exec_failure_preserves_session_with_error_result() {
    let tmp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();

    let session = csa_session::create_session(project_root, Some("test"), None, Some("codex"))
        .expect("session creation must succeed");

    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id).unwrap();
    assert!(session_dir.exists());

    {
        let mut guard = SessionCleanupGuard::new(session_dir.clone());
        let error = anyhow::anyhow!("spawn failed: command not found");
        write_pre_exec_error_result(project_root, &session.meta_session_id, "codex", &error);
        guard.defuse();
    }

    assert!(
        session_dir.exists(),
        "session directory must be preserved after pre-exec failure"
    );

    let loaded = csa_session::load_result(project_root, &session.meta_session_id)
        .expect("load_result must not error")
        .expect("result.toml must exist after pre-exec failure");
    assert_eq!(loaded.status, "failure");
    assert!(loaded.summary.starts_with("pre-exec:"));
    assert!(loaded.summary.contains("spawn failed"));
}

/// Verify that `write_pre_exec_error_result` produces a result.toml with
/// status = "failure" and a summary prefixed with "pre-exec:".
#[test]
fn pre_exec_error_writes_failure_result() {
    let tmp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();

    let session = csa_session::create_session(project_root, Some("test"), None, Some("codex"))
        .expect("session creation must succeed");

    let error = anyhow::anyhow!("tool binary not found in PATH");
    write_pre_exec_error_result(project_root, &session.meta_session_id, "codex", &error);

    let loaded = csa_session::load_result(project_root, &session.meta_session_id)
        .expect("load_result must not error")
        .expect("result.toml must exist");
    assert_eq!(loaded.status, "failure", "status must be failure");
    assert_eq!(loaded.exit_code, 1, "exit_code must be 1");
    assert!(
        loaded.summary.starts_with("pre-exec:"),
        "summary must start with 'pre-exec:', got: {}",
        loaded.summary
    );
    assert!(
        loaded.summary.contains("tool binary not found"),
        "summary must contain the error message, got: {}",
        loaded.summary
    );
    assert_eq!(loaded.tool, "codex");
    assert!(loaded.artifacts.is_empty());
}
