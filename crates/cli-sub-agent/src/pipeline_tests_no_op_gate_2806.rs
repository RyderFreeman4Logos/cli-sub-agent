//! Regressions for #2806 receipt freshness and historical completion prose.

use super::*;

#[tokio::test]
async fn stale_legacy_receipt_does_not_satisfy_dirty_sa_completion() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");
    // This legacy path can contain a receipt from a completed prior turn. It
    // must never satisfy the current turn's positive-completion requirement.
    write_result_sidecar(
        &session_dir,
        r#"[result]
status = "success"
summary = "previous turn passed"
"#,
    );

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(15);
    let mut ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    ctx.changed_paths = vec!["src/dirty.rs".to_string()];
    let mut result = build_test_result("Applied current-turn edits without a receipt.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 1);
    assert!(
        persisted
            .summary
            .contains("dirty SA-mode run lacks a positive structured completion signal"),
        "stale legacy receipt must not preserve current dirty-work success: {}",
        persisted.summary
    );
}

#[tokio::test]
async fn current_structured_success_suppresses_historical_omitted_work_prose() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");
    write_current_turn_result_sidecar(
        &session_dir,
        session.turn_count,
        r#"[result]
status = "success"
summary = "current turn completed"
"#,
    );

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(15);
    let mut ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    ctx.changed_paths = vec!["src/fixed.rs".to_string()];
    let mut result = build_test_result(
        "This turn fixed the previously omitted tests and commit; all current work is complete.",
    );

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(0));
}
