// --- classify_join_error tests ---

#[tokio::test]
async fn test_classify_join_error_broken_pipe_message() {
    let handle = tokio::task::spawn(async {
        panic!("failed printing to stderr: Broken pipe (os error 32)")
    });
    let join_err = handle.await.unwrap_err();
    let err = super::classify_join_error(join_err);
    let msg = err.to_string();
    assert!(
        msg.contains("tool process terminated unexpectedly"),
        "broken pipe should get a clean message, got: {msg}"
    );
    assert!(
        msg.contains("broken pipe"),
        "message should mention broken pipe, got: {msg}"
    );
}

#[tokio::test]
async fn test_classify_join_error_generic_panic() {
    let handle = tokio::task::spawn(async { panic!("something else went wrong") });
    let join_err = handle.await.unwrap_err();
    let err = super::classify_join_error(join_err);
    let msg = err.to_string();
    assert!(
        msg.contains("task panicked"),
        "generic panic should say 'task panicked', got: {msg}"
    );
    assert!(
        msg.contains("something else went wrong"),
        "should include panic message, got: {msg}"
    );
}

// --- build_summary tests (moved from transport.rs) ---

#[test]
fn test_build_summary_uses_last_stdout_line_on_success() {
    let stdout = "line1\nfinal line\n";
    let summary = super::build_summary(stdout, "", 0);
    assert_eq!(summary, "final line");
}

#[test]
fn test_build_summary_uses_stdout_on_failure_when_present() {
    let stdout = "details\nreason from stdout\n";
    let summary = super::build_summary(stdout, "stderr message", 2);
    assert_eq!(summary, "reason from stdout");
}

#[test]
fn test_build_summary_falls_back_to_stderr_on_failure() {
    let summary = super::build_summary("\n", "stderr reason\n", 3);
    assert_eq!(summary, "stderr reason");
}

#[test]
fn test_build_summary_ignores_csa_section_markers() {
    let stdout = "Valid summary line\n<!-- CSA:SECTION:summary:END -->\n";
    let summary = super::build_summary(stdout, "", 0);
    assert_eq!(summary, "Valid summary line");
}

#[test]
fn test_build_summary_falls_back_to_exit_code_when_no_output() {
    let summary = super::build_summary("", "   \n", -1);
    assert_eq!(summary, "exit code -1");
}

// --- 3-phase Gemini fallback chain integration tests ---

/// Phase 1 (OAuth) fails with rate-limit, Phase 2 (API key, same model) succeeds.
/// Verifies: model log shows [inherit, inherit], API key was injected on attempt 2.
#[tokio::test]
async fn test_gemini_3phase_oauth_fails_apikey_same_model_succeeds() {
    let (_temp, mut env, model_log_path) = setup_fake_gemini_environment(2);
    // Enable the 3-phase chain by providing API key fallback and OAuth auth mode.
    env.insert("_CSA_API_KEY_FALLBACK".to_string(), "test-key-3phase".to_string());
    env.insert("_CSA_GEMINI_AUTH_MODE".to_string(), "oauth".to_string());

    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });

    let result = transport
        .execute_in(
            "test 3phase oauth-fail apikey-succeed",
            std::path::Path::new("/tmp"),
            Some(&env),
            StreamMode::BufferOnly,
            30,
        )
        .await
        .expect("execute_in should succeed on attempt 2 (API key, same model)");

    assert_eq!(result.execution.exit_code, 0);
    assert!(
        result.execution.output.contains("ok attempt=2"),
        "expected success on attempt 2, got: {}",
        result.execution.output
    );

    // Model log: both attempts keep original model (inherit)
    let models = read_model_log(&model_log_path);
    assert_eq!(
        models,
        vec!["inherit".to_string(), "inherit".to_string()],
        "phase 1 and 2 should both use original model"
    );

    // Auth log: attempt 1 = oauth, attempt 2 = api_key
    let auths = read_auth_log(&model_log_path);
    assert_eq!(
        auths,
        vec!["oauth".to_string(), "api_key".to_string()],
        "phase 1 should use OAuth, phase 2 should inject API key"
    );
}

/// Phase 1 (OAuth) and Phase 2 (API key, same model) both fail.
/// Phase 3 (API key, flash model) succeeds.
/// Verifies: model log shows [inherit, inherit, flash], API key injected on attempts 2,3.
#[tokio::test]
async fn test_gemini_3phase_all_oauth_and_apikey_same_fail_flash_succeeds() {
    let (_temp, mut env, model_log_path) = setup_fake_gemini_environment(3);
    env.insert("_CSA_API_KEY_FALLBACK".to_string(), "test-key-3phase".to_string());
    env.insert("_CSA_GEMINI_AUTH_MODE".to_string(), "oauth".to_string());

    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });

    let result = transport
        .execute_in(
            "test 3phase all-fail-until-flash",
            std::path::Path::new("/tmp"),
            Some(&env),
            StreamMode::BufferOnly,
            30,
        )
        .await
        .expect("execute_in should succeed on attempt 3 (API key, flash model)");

    assert_eq!(result.execution.exit_code, 0);
    assert!(
        result.execution.output.contains("ok attempt=3"),
        "expected success on attempt 3, got: {}",
        result.execution.output
    );

    // Model log: phase 1,2 keep original, phase 3 switches to flash
    let models = read_model_log(&model_log_path);
    assert_eq!(
        models,
        vec![
            "inherit".to_string(),
            "inherit".to_string(),
            "gemini-3-flash-preview".to_string(),
        ],
        "phase 3 should downgrade to flash model"
    );

    // Auth log: attempt 1 = oauth, attempts 2,3 = api_key
    let auths = read_auth_log(&model_log_path);
    assert_eq!(
        auths,
        vec![
            "oauth".to_string(),
            "api_key".to_string(),
            "api_key".to_string(),
        ],
        "phase 2 and 3 should both use API key auth"
    );
}

/// All 3 phases fail with rate-limit. Verifies: returns error, model log shows
/// all 3 attempts, API key was injected on attempts 2 and 3.
#[tokio::test]
async fn test_gemini_3phase_all_fail_returns_last_error() {
    let (_temp, mut env, model_log_path) = setup_fake_gemini_environment(99);
    env.insert("_CSA_API_KEY_FALLBACK".to_string(), "test-key-3phase".to_string());
    env.insert("_CSA_GEMINI_AUTH_MODE".to_string(), "oauth".to_string());

    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });

    let result = transport
        .execute_in(
            "test 3phase all-fail",
            std::path::Path::new("/tmp"),
            Some(&env),
            StreamMode::BufferOnly,
            30,
        )
        .await
        .expect("execute_in should return final failed attempt result");

    // Final result is the last (3rd) attempt failure
    assert_ne!(result.execution.exit_code, 0);
    assert!(
        result.execution.stderr_output.contains("QUOTA_EXHAUSTED"),
        "expected QUOTA_EXHAUSTED in stderr, got: {}",
        result.execution.stderr_output
    );

    // Model log: all 3 phases attempted
    let models = read_model_log(&model_log_path);
    assert_eq!(
        models,
        vec![
            "inherit".to_string(),
            "inherit".to_string(),
            "gemini-3-flash-preview".to_string(),
        ],
        "retry loop should execute all 3 phases before giving up"
    );

    // Auth log: attempt 1 = oauth, attempts 2,3 = api_key
    let auths = read_auth_log(&model_log_path);
    assert_eq!(
        auths,
        vec![
            "oauth".to_string(),
            "api_key".to_string(),
            "api_key".to_string(),
        ],
        "API key should be injected on attempts 2 and 3"
    );
}
