#[tokio::test]
async fn test_execute_stops_after_max_attempts_and_returns_last_failure() {
    let (temp, env, model_log_path) = setup_fake_gemini_environment(99);
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let session = build_test_meta_session(temp.path().to_str().expect("utf8 temp path"));
    let options = TransportOptions {
        stream_mode: StreamMode::BufferOnly,
        idle_timeout_seconds: 30,
        acp_crash_max_attempts: 2,
        initial_response_timeout: super::ResolvedTimeout(None),
        liveness_dead_seconds: 30,
        stdin_write_timeout_seconds: 30,
        acp_init_timeout_seconds: 30,
        termination_grace_period_seconds: 1,
        output_spool: None,
        output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
        output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
        error_marker_scan_enabled: true,
        setting_sources: None,
        sandbox: None,
        thinking_budget: None,
        subtree_pin: None,
        allow_git_push: false,
        no_post_exec_gate: false,
        cancellation: None,
    };

    let result = transport
        .execute("test retry loop", None, &session, Some(&env), options)
        .await
        .expect("execute should return final failed attempt result");

    assert_ne!(result.execution.exit_code, 0);
    assert!(
        result.execution.stderr_output.contains("Too Many Requests"),
        "unexpected stderr: {}",
        result.execution.stderr_output
    );
    let models = read_model_log(&model_log_path);
    // All transport retry phases preserve the configured model.
    assert_eq!(
        models,
        vec![
            "inherit".to_string(),
            "inherit".to_string(),
            "inherit".to_string()
        ],
        "retry loop should stop after 3 attempts"
    );
}
