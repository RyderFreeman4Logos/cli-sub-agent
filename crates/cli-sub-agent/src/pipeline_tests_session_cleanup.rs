use super::*;
use crate::session_guard::{SessionCleanupGuard, write_pre_exec_error_result};
use crate::test_session_sandbox::ScopedSessionSandbox;
use chrono::Utc;
use csa_config::GlobalConfig;
use csa_core::types::OutputFormat;
use csa_executor::Executor;
use std::fs;
use std::path::Path;

const STATE_DIR_CAP_TEST_BYTES: u64 = 2 * 1024 * 1024;

fn state_dir_error_global_config() -> GlobalConfig {
    toml::from_str(
        r#"
        [state_dir]
        max_size_mb = 1
        scan_interval_seconds = 0
        on_exceed = "error"
        "#,
    )
    .expect("parse global config")
}

fn seed_state_dir_over_cap() {
    let state_dir = csa_config::paths::state_dir().expect("state dir");
    fs::create_dir_all(&state_dir).expect("create state dir");
    let filler = std::fs::File::create(state_dir.join("oversized.bin")).expect("create filler");
    filler
        .set_len(STATE_DIR_CAP_TEST_BYTES)
        .expect("extend filler file");
}

fn assert_state_dir_cap_failure_result(project_root: &Path, session_id: &str) {
    let session_dir = csa_session::get_session_dir(project_root, session_id).expect("session dir");
    assert!(
        session_dir.exists(),
        "session directory must survive pre-exec state-dir cap failure"
    );

    let result_path = session_dir.join("result.toml");
    assert!(result_path.exists(), "result.toml must be written");

    let raw_result = fs::read_to_string(&result_path).expect("read result.toml");
    assert!(
        raw_result.contains("/ 1 MB cap exceeded"),
        "result.toml must record the configured cap: {raw_result}"
    );
    assert!(
        raw_result.contains("on_exceed = \"error\""),
        "result.toml must record on_exceed=error: {raw_result}"
    );

    let loaded = csa_session::load_result(project_root, session_id)
        .expect("load_result must not error")
        .expect("result.toml must exist");
    assert_eq!(loaded.status, "failure");
    assert!(loaded.summary.starts_with("pre-exec:"));
    assert!(
        loaded.summary.contains("cap exceeded"),
        "summary must mention cap failure, got: {}",
        loaded.summary
    );
}

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

#[tokio::test]
async fn state_dir_cap_failure_persists_result_for_fresh_spawn() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sandbox = ScopedSessionSandbox::new(&tmp).await;
    sandbox.track_env("CSA_SESSION_ID");
    // SAFETY: test-scoped env mutation while ScopedSessionSandbox holds TEST_ENV_LOCK.
    unsafe { std::env::remove_var("CSA_SESSION_ID") };
    let project_root = tmp.path();
    let global = state_dir_error_global_config();
    let executor = Executor::Opencode {
        model_override: None,
        agent: None,
        thinking_budget: None,
    };

    seed_state_dir_over_cap();

    let err = match execute_with_session_and_meta(
        &executor,
        &ToolName::Opencode,
        "trip the state-dir cap",
        OutputFormat::Json,
        None,
        false,
        Some("fresh-state-dir-cap".to_string()),
        None,
        project_root,
        None,
        None,
        Some("run"),
        None,
        None,
        csa_process::StreamMode::BufferOnly,
        DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        None,
        None,
        Some(&global),
        false,
        false,
        &[],
        &[],
    )
    .await
    {
        Ok(_) => panic!("state-dir cap must reject fresh spawn"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("cap exceeded"),
        "fresh-spawn error should mention the state-dir cap: {err:#}"
    );

    let sessions = csa_session::list_sessions(project_root, None).expect("list sessions");
    assert_eq!(
        sessions.len(),
        1,
        "fresh-spawn pre-exec failure must preserve the new session"
    );
    assert_state_dir_cap_failure_result(project_root, &sessions[0].meta_session_id);
}

#[tokio::test]
async fn state_dir_cap_failure_overwrites_stale_result_for_resume() {
    let tmp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let global = state_dir_error_global_config();
    let executor = Executor::Opencode {
        model_override: None,
        agent: None,
        thinking_budget: None,
    };

    seed_state_dir_over_cap();

    let session =
        csa_session::create_session(project_root, Some("resume target"), None, Some("opencode"))
            .expect("create resume session");
    let stale_summary = "stale prior result";
    let stale_manager_summary = "stale sidecar manager summary";
    csa_session::save_result(
        project_root,
        &session.meta_session_id,
        &csa_session::SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: stale_summary.to_string(),
            tool: "opencode".to_string(),
            started_at: Utc::now(),
            completed_at: Utc::now(),
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
            manager_fields: csa_session::SessionManagerFields {
                report: Some(
                    toml::toml! {
                        summary = stale_manager_summary
                    }
                    .into(),
                ),
                ..Default::default()
            },
        },
    )
    .expect("seed stale result");

    let err = match execute_with_session_and_meta(
        &executor,
        &ToolName::Opencode,
        "trip the state-dir cap on resume",
        OutputFormat::Json,
        Some(session.meta_session_id.clone()),
        false,
        Some("resume-state-dir-cap".to_string()),
        None,
        project_root,
        None,
        None,
        Some("run"),
        None,
        None,
        csa_process::StreamMode::BufferOnly,
        DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        None,
        None,
        Some(&global),
        false,
        false,
        &[],
        &[],
    )
    .await
    {
        Ok(_) => panic!("state-dir cap must reject resume"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("cap exceeded"),
        "resume error should mention the state-dir cap: {err:#}"
    );

    assert_state_dir_cap_failure_result(project_root, &session.meta_session_id);
    let loaded = csa_session::load_result(project_root, &session.meta_session_id)
        .expect("load_result must not error")
        .expect("result.toml must exist");
    assert_ne!(
        loaded.summary, stale_summary,
        "resume path must overwrite stale result.toml"
    );
    assert!(
        loaded.manager_fields.as_sidecar().is_none(),
        "cap-error overwrite must not rehydrate stale manager sidecar fields"
    );
    assert!(
        loaded
            .artifacts
            .iter()
            .all(|artifact| artifact.path != csa_session::CONTRACT_RESULT_ARTIFACT_PATH),
        "cap-error overwrite must not keep advertising the stale manager sidecar"
    );
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    assert!(
        !csa_session::contract_result_path(&session_dir).exists(),
        "cap-error overwrite must clear the stale manager sidecar file"
    );
}
