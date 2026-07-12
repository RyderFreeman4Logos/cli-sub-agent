use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use std::fs;

/// Run the session pipeline far enough to exercise worktree-write-lock
/// acquisition, varying only the knobs each lock test cares about: `task_type`
/// (`run` / `reviewer_sub_session` / `debate`), `session_arg` (fresh vs resume),
/// and `readonly_project_root` (the worktree-mutation signal, #1828). Every
/// other argument is fixed to an inert test value.
async fn run_pipeline_for_worktree_lock_test(
    project_root: &std::path::Path,
    task_type: Option<&str>,
    session_arg: Option<String>,
    readonly_project_root: bool,
) -> anyhow::Result<crate::pipeline::SessionExecutionResult> {
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: csa_executor::CodexRuntimeMetadata::from_transport(
            csa_executor::CodexTransport::Acp,
        ),
    };
    execute_with_session_and_meta(
        &executor,
        &ToolName::Codex,
        "lock-test prompt",
        csa_core::types::OutputFormat::Json,
        session_arg,
        false,
        None,
        None,
        project_root,
        None,
        None,
        None,
        task_type,
        None,
        None,
        csa_process::StreamMode::BufferOnly,
        DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        None,
        None,
        None,
        None,
        false,
        readonly_project_root,
        &[],
        &[],
        None, // error_marker_scan_override: defer to marker/config (#1745/#1847)
        false,
        &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV,
    )
    .await
}

fn acquire_active_holder_worktree_lock(
    project_root: &std::path::Path,
) -> (String, csa_lock::WorktreeWriteLock) {
    let holder =
        csa_session::create_session(project_root, Some("holder"), None, Some("codex")).unwrap();
    let lock = csa_lock::acquire_worktree_write_lock(
        project_root,
        &holder.meta_session_id,
        &[],
        |_| false,
        |_| false,
        |_| false,
    )
    .expect("holder worktree write lock should succeed");
    (holder.meta_session_id, lock)
}

/// Assert the pipeline failed fast on the per-worktree write lock, surfacing the
/// non-lineage holder session id and serialize guidance (#1672).
fn assert_worktree_write_lock_blocked(
    execution: anyhow::Result<crate::pipeline::SessionExecutionResult>,
    project_root: &std::path::Path,
    expected_holder_session_id: &str,
) {
    let err = match execution {
        Ok(_) => panic!("held worktree write lock must reject non-lineage writer"),
        Err(err) => err.to_string(),
    };
    assert!(
        err.contains("concurrent write session blocked"),
        "unexpected error: {err}"
    );
    assert!(
        err.contains(expected_holder_session_id),
        "missing holder session id: {err}"
    );
    assert!(
        err.contains(&project_root.display().to_string()),
        "missing worktree path: {err}"
    );
    assert!(
        err.contains("sequentially"),
        "missing serialize guidance: {err}"
    );
}

/// Assert the pipeline got PAST the worktree-write-lock gate (no contention with
/// the held lock) and failed only on its own per-session lock — the expected
/// outcome for read-only sessions and in-lineage re-entries.
fn assert_blocked_on_session_lock_not_worktree(
    execution: anyhow::Result<crate::pipeline::SessionExecutionResult>,
) {
    let err = match execution {
        Ok(_) => panic!("held per-session lock must reject the resume attempt"),
        Err(err) => err.to_string(),
    };
    assert!(
        err.contains("Failed to acquire lock for session"),
        "unexpected error: {err}"
    );
    assert!(
        !err.contains("concurrent write session blocked"),
        "read-only / in-lineage session must not be blocked by worktree write lock: {err}"
    );
}

#[tokio::test]
async fn execute_with_session_and_meta_does_not_persist_runtime_binary_when_lock_is_held() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path();

    let session =
        csa_session::create_session(project_root, Some("resume-target"), None, Some("codex"))
            .unwrap();
    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id).unwrap();
    let metadata_path = session_dir.join(csa_session::metadata::METADATA_FILE_NAME);
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "codex".to_string(),
        tool_locked: true,
        runtime_binary: Some("codex".to_string()),
    };
    fs::write(&metadata_path, toml::to_string_pretty(&metadata).unwrap()).unwrap();

    let _lock = csa_lock::acquire_lock(&session_dir, "codex", "active resume winner").unwrap();
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: csa_executor::CodexRuntimeMetadata::from_transport(
            csa_executor::CodexTransport::Acp,
        ),
    };

    let execution = execute_with_session_and_meta(
        &executor,
        &ToolName::Codex,
        "resume prompt",
        csa_core::types::OutputFormat::Json,
        Some(session.meta_session_id.clone()),
        false,
        None,
        None,
        project_root,
        None, // config
        None, // extra_env
        None, // subtree_pin (#1741)
        None, // task_type
        None, // tier_name
        None, // context_load_options
        csa_process::StreamMode::BufferOnly,
        DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        None,
        None,
        None,
        None,
        false,
        false,
        &[],
        &[],
        None,  // error_marker_scan_override: defer to marker/config (#1745/#1847)
        false, // cli_no_hook_bypass_scan: no CLI flag here; defer to config
        &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV,
    )
    .await;
    let err = match execution {
        Ok(_) => panic!("held session lock must reject the losing resume attempt"),
        Err(err) => err,
    };

    let rendered = err.to_string();
    assert!(
        rendered.contains("Failed to acquire lock"),
        "unexpected error: {err:#}"
    );
    assert!(
        rendered.contains(&session_dir.join("locks/codex.lock").display().to_string()),
        "missing lock path diagnostic: {rendered}"
    );
    assert!(
        rendered.contains(&format!("Session locked by PID {}", std::process::id())),
        "missing lock owner PID: {rendered}"
    );
    assert!(
        rendered.contains("pid_status: alive"),
        "missing lock-owner liveness: {rendered}"
    );
    assert!(
        rendered.contains("reason: active resume winner"),
        "missing lock reason: {rendered}"
    );

    let persisted = toml::from_str::<csa_session::metadata::SessionMetadata>(
        &fs::read_to_string(&metadata_path).unwrap(),
    )
    .unwrap();
    assert_eq!(
        persisted.runtime_binary.as_deref(),
        Some("codex"),
        "lock loser must not overwrite the winner's runtime_binary"
    );
}

#[tokio::test]
async fn run_writer_fails_fast_when_worktree_write_lock_is_held() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path();

    let (holder_session_id, _worktree_lock) = acquire_active_holder_worktree_lock(project_root);

    let execution =
        run_pipeline_for_worktree_lock_test(project_root, Some("run"), None, false).await;
    assert_worktree_write_lock_blocked(execution, project_root, &holder_session_id);
}

#[tokio::test]
async fn review_fix_fails_fast_when_worktree_write_lock_is_held() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path();

    let (holder_session_id, _worktree_lock) = acquire_active_holder_worktree_lock(project_root);

    // `csa review --fix` runs as a `reviewer_sub_session` with a writable
    // project root (`readonly_project_root == false`), so it mutates the shared
    // worktree and must contend on the same per-worktree write lock a concurrent
    // `csa run` writer holds (#1828) — previously it slipped past the lock.
    let execution = run_pipeline_for_worktree_lock_test(
        project_root,
        Some("reviewer_sub_session"),
        None,
        false,
    )
    .await;
    assert_worktree_write_lock_blocked(execution, project_root, &holder_session_id);
}

#[tokio::test]
async fn debate_write_mode_fails_fast_when_worktree_write_lock_is_held() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path();

    let (holder_session_id, _worktree_lock) = acquire_active_holder_worktree_lock(project_root);

    // A write-mode debate (`readonly_project_root == false`) can mutate the
    // worktree and must serialize against concurrent writers (#1828).
    let execution =
        run_pipeline_for_worktree_lock_test(project_root, Some("debate"), None, false).await;
    assert_worktree_write_lock_blocked(execution, project_root, &holder_session_id);
}

#[tokio::test]
async fn run_commit_child_reenters_under_ancestor_worktree_write_lock() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path();

    let holder =
        csa_session::create_session(project_root, Some("holder"), None, Some("codex")).unwrap();
    let _worktree_lock = csa_lock::acquire_worktree_write_lock(
        project_root,
        &holder.meta_session_id,
        &[],
        |_| false,
        |_| false,
        |_| false,
    )
    .expect("ancestor worktree write lock should succeed");
    let child = csa_session::create_session(
        project_root,
        Some("skill:commit"),
        Some(&holder.meta_session_id),
        Some("codex"),
    )
    .unwrap();
    let child_dir = csa_session::get_session_dir(project_root, &child.meta_session_id).unwrap();
    let _child_session_lock =
        csa_lock::acquire_lock(&child_dir, "codex", "active commit child").unwrap();

    let execution = run_pipeline_for_worktree_lock_test(
        project_root,
        Some("run"),
        Some(child.meta_session_id),
        false,
    )
    .await;

    assert_blocked_on_session_lock_not_worktree(execution);
}

#[tokio::test]
async fn run_child_of_different_parent_fails_fast_when_worktree_write_lock_is_held() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path();

    let (holder_session_id, _worktree_lock) = acquire_active_holder_worktree_lock(project_root);
    let other_parent =
        csa_session::create_session(project_root, Some("other parent"), None, Some("codex"))
            .unwrap();
    let child = csa_session::create_session(
        project_root,
        Some("skill:commit"),
        Some(&other_parent.meta_session_id),
        Some("codex"),
    )
    .unwrap();

    let execution = run_pipeline_for_worktree_lock_test(
        project_root,
        Some("run"),
        Some(child.meta_session_id),
        false,
    )
    .await;

    assert_worktree_write_lock_blocked(execution, project_root, &holder_session_id);
}

#[tokio::test]
async fn readonly_review_is_not_blocked_by_worktree_write_lock() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path();

    let session =
        csa_session::create_session(project_root, Some("review-target"), None, Some("codex"))
            .unwrap();
    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id).unwrap();
    // Hold the per-session lock so the pipeline has a deterministic failure point
    // AFTER the worktree-lock gate.
    let _session_lock = csa_lock::acquire_lock(&session_dir, "codex", "active review").unwrap();
    let (_holder_session_id, _worktree_lock) = acquire_active_holder_worktree_lock(project_root);

    // A read-only review (`readonly_project_root == true`) cannot mutate the
    // worktree, acquires no worktree lock, and must not contend with the holder.
    let execution = run_pipeline_for_worktree_lock_test(
        project_root,
        Some("reviewer_sub_session"),
        Some(session.meta_session_id.clone()),
        true,
    )
    .await;
    assert_blocked_on_session_lock_not_worktree(execution);
}

#[tokio::test]
async fn readonly_debate_is_not_blocked_by_worktree_write_lock() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path();

    let session =
        csa_session::create_session(project_root, Some("debate-target"), None, Some("codex"))
            .unwrap();
    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id).unwrap();
    let _session_lock = csa_lock::acquire_lock(&session_dir, "codex", "active debate").unwrap();
    let (_holder_session_id, _worktree_lock) = acquire_active_holder_worktree_lock(project_root);

    // A read-only debate (`readonly_project_root == true`) acquires no worktree
    // lock → it is not blocked by the holder.
    let execution = run_pipeline_for_worktree_lock_test(
        project_root,
        Some("debate"),
        Some(session.meta_session_id.clone()),
        true,
    )
    .await;
    assert_blocked_on_session_lock_not_worktree(execution);
}

#[tokio::test]
async fn review_fix_reenters_under_ancestor_worktree_write_lock() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path();

    // Ancestor session holds the per-worktree write lock.
    let holder =
        csa_session::create_session(project_root, Some("holder"), None, Some("codex")).unwrap();
    let _worktree_lock = csa_lock::acquire_worktree_write_lock(
        project_root,
        &holder.meta_session_id,
        &[],
        |_| false,
        |_| false,
        |_| false,
    )
    .expect("ancestor worktree write lock should succeed");

    // A `--fix` child WITHIN the holder's lineage must re-enter the lock, not
    // fail fast (#1828 reuses #1672's lineage re-entry path).
    let child = csa_session::create_session(
        project_root,
        Some("fix-child"),
        Some(&holder.meta_session_id),
        Some("codex"),
    )
    .unwrap();
    let child_dir = csa_session::get_session_dir(project_root, &child.meta_session_id).unwrap();
    // Hold the child's per-session lock so the pipeline stops AFTER re-entering
    // the worktree lock — proving re-entry rather than fail-fast contention.
    let _child_session_lock = csa_lock::acquire_lock(&child_dir, "codex", "active fix").unwrap();

    let execution = run_pipeline_for_worktree_lock_test(
        project_root,
        Some("reviewer_sub_session"),
        Some(child.meta_session_id.clone()),
        false,
    )
    .await;
    assert_blocked_on_session_lock_not_worktree(execution);
}
