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
async fn current_receipt_suppresses_historical_agent_stdout_prose() {
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
    let mut result = build_test_result("Current-turn completion receipt reports success.");
    result.output = [
        r#"{"type":"thread.started","thread_id":"current-success"}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"Initial gate status was unknown; this turn reran the gate and it now reports PASS. This turn also fixed the previously omitted tests and commit; all current work is complete."}}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#,
    ]
    .join("\n");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(result.exit_code, 0);
    assert!(
        result.csa_gate_failure.is_none(),
        "fresh success receipt and resolved agent prose must not trigger a CSA gate"
    );
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(0));
    assert!(
        !persisted.summary.starts_with("worker blocked:"),
        "resolved historical stdout must not rewrite the result as blocked: {}",
        persisted.summary
    );
}

#[tokio::test]
async fn current_receipt_does_not_suppress_unresolved_agent_stdout_diagnostic() {
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
    ctx.changed_paths = vec!["src/unresolved.rs".to_string()];
    let mut result = build_test_result("Current-turn completion receipt reports success.");
    result.output = [
        r#"{"type":"thread.started","thread_id":"current-unresolved"}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"zsh: read-only variable: status"}}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#,
    ]
    .join("\n");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(result.exit_code, 1);
    assert!(
        result.csa_gate_failure.is_some(),
        "unresolved agent diagnostic must trigger a CSA gate"
    );
    assert_eq!(persisted.exit_code, 1);
    assert!(persisted.summary.contains("worker blocked"));
}

#[tokio::test]
async fn stale_next_turn_receipt_with_prior_attempt_nonce_does_not_satisfy_dirty_sa_completion() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");
    assert!(
        crate::pipeline::result_contract::clear_expected_result_artifacts_for_prompt(
            crate::pipeline::result_contract::RESULT_TOML_PATH_CONTRACT_MARKER,
            &session_dir,
            session.turn_count,
        ),
        "retry must clear and bind the next-turn receipt before provider execution"
    );
    let current_attempt =
        crate::pipeline::result_contract::current_result_attempt_nonce(&session_dir)
            .expect("retry preflight must persist its attempt nonce");
    assert_ne!(current_attempt, "stale-attempt");
    let stale_path = csa_session::next_turn_contract_result_path(&session_dir, session.turn_count);
    std::fs::create_dir_all(stale_path.parent().expect("turn result parent"))
        .expect("create turn result parent");
    std::fs::write(
        stale_path,
        r#"[result]
status = "success"
summary = "interrupted prior attempt"
attempt_nonce = "stale-attempt"
"#,
    )
    .expect("simulate prior attempt receipt arriving after retry preflight");

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
        true,
        true,
    );
    ctx.changed_paths = vec!["src/dirty.rs".to_string()];
    let mut result = build_test_result("normal retry output");

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
            .contains("dirty SA-mode run lacks a positive structured completion signal")
    );
}

#[tokio::test]
async fn stderr_only_lost_shell_status_blocks_successful_provider_result() {
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
    let ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        true,
        true,
    );
    let mut result = build_test_result("normal provider summary");
    result.stderr_output = "zsh: read-only variable: status".to_string();

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 1);
    assert!(persisted.summary.contains("worker blocked"));
}
