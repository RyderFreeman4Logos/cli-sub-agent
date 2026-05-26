use super::*;

#[tokio::test]
async fn no_op_gate_does_not_trigger_for_non_run_task_type() {
    for task_type in &["review", "debate", "plan"] {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _sandbox = ScopedSessionSandbox::new(&tmp).await;
        let project_root = tmp.path();
        let mut session =
            create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
        let session_dir =
            csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

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
        ctx.task_type = Some(task_type);
        let mut result = build_test_result("Review completed successfully.");

        process_execution_result(ctx, &mut session, &mut result)
            .await
            .expect("process_execution_result");

        let persisted = load_result(project_root, &session.meta_session_id)
            .expect("load")
            .expect("result exists");
        assert_eq!(
            persisted.exit_code, 0,
            "gate must not fire for task_type={task_type}"
        );
        assert_eq!(
            persisted.status,
            SessionResult::status_from_exit_code(0),
            "status must remain success for task_type={task_type}"
        );
        assert!(
            !persisted.summary.starts_with("no-op exit detected"),
            "summary must NOT be prefixed for task_type={task_type}, got: {}",
            persisted.summary
        );
    }
}

#[tokio::test]
async fn no_op_gate_still_triggers_for_run_task_type() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(10);
    let mut ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    ctx.task_type = Some("run");
    let mut result = build_test_result("I'll start by exploring.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 1, "gate must fire for task_type=run");
    assert_eq!(
        persisted.status,
        SessionResult::status_from_exit_code(1),
        "status must be failure for task_type=run"
    );
    assert!(
        persisted.summary.starts_with("no-op exit detected"),
        "summary must be prefixed for task_type=run, got: {}",
        persisted.summary
    );
}

#[tokio::test]
async fn no_op_gate_triggers_when_task_type_is_none() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(10);
    let mut ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    ctx.task_type = None;
    let mut result = build_test_result("I'll start by exploring.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(
        persisted.exit_code, 1,
        "gate must fire when task_type is None"
    );
    assert_eq!(
        persisted.status,
        SessionResult::status_from_exit_code(1),
        "status must be failure when task_type is None"
    );
    assert!(
        persisted.summary.starts_with("no-op exit detected"),
        "summary must be prefixed when task_type is None, got: {}",
        persisted.summary
    );
}
