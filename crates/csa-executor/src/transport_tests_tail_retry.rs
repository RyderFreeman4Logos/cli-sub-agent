#[test]
fn test_should_retry_gemini_rate_limited_until_final_attempt() {
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let execution = ExecutionResult {
        summary: "failed".to_string(),
        output: String::new(),
        stderr_output: "HTTP 429 Too Many Requests".to_string(),
        exit_code: 1,
        peak_memory_mb: None,
    };

    assert!(
        transport
            .should_retry_gemini_rate_limited(&execution, 1, None)
            .is_some()
    );
    assert!(
        transport
            .should_retry_gemini_rate_limited(&execution, 2, None)
            .is_some()
    );
    assert!(
        transport
            .should_retry_gemini_rate_limited(&execution, 3, None)
            .is_none()
    );
}

#[test]
fn test_should_not_retry_on_success_exit_code() {
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let execution = ExecutionResult {
        summary: "ok".to_string(),
        output: "429".to_string(),
        stderr_output: String::new(),
        exit_code: 0,
        peak_memory_mb: None,
    };
    assert!(
        transport
            .should_retry_gemini_rate_limited(&execution, 1, None)
            .is_none()
    );
}

#[test]
fn test_should_retry_generic_quota_exhausted_marker() {
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let execution = ExecutionResult {
        summary: "failed".to_string(),
        output: String::new(),
        stderr_output: "reason: 'QUOTA_EXHAUSTED'".to_string(),
        exit_code: 1,
        peak_memory_mb: None,
    };
    assert!(
        transport
            .should_retry_gemini_rate_limited(&execution, 1, None)
            .is_some()
    );
    assert_eq!(
        detect_gemini_permanent_quota_exhaustion_result(&execution),
        None
    );
}

#[test]
fn test_should_retry_transient_429_too_many_requests() {
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let execution = ExecutionResult {
        summary: "failed".to_string(),
        output: String::new(),
        stderr_output: "HTTP 429 Too Many Requests".to_string(),
        exit_code: 1,
        peak_memory_mb: None,
    };
    assert!(
        transport
            .should_retry_gemini_rate_limited(&execution, 1, None)
            .is_some()
    );
    assert_eq!(
        detect_gemini_permanent_quota_exhaustion_result(&execution),
        None
    );
}

#[tokio::test]
async fn test_execute_in_permanent_quota_exhaustion_does_not_api_key_fallback() {
    let (_temp, mut env, model_log_path) = setup_fake_gemini_environment(99);
    env.insert(
        "_CSA_API_KEY_FALLBACK".to_string(),
        "fallback-key".to_string(),
    );
    env.insert("_CSA_GEMINI_AUTH_MODE".to_string(), "oauth".to_string());
    env.insert("_CSA_NO_FLASH_FALLBACK".to_string(), "1".to_string());
    env.insert(
        "CSA_FAKE_GEMINI_FAILURE_REASON".to_string(),
        "RESOURCE_EXHAUSTED: QUOTA_EXHAUSTED monthly spending cap reached".to_string(),
    );
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });

    let result = transport
        .execute_in(
            "permanent quota exhaustion",
            std::path::Path::new("/tmp"),
            Some(&env),
            StreamMode::BufferOnly,
            30,
            super::ResolvedTimeout(None),
        )
        .await
        .expect("permanent quota exhaustion should return a failed result");

    assert_eq!(result.execution.exit_code, 1);
    assert!(
        result.execution.summary.starts_with("tool_exhausted: gemini-cli"),
        "unexpected summary: {}",
        result.execution.summary
    );
    assert_eq!(
        read_auth_log(&model_log_path),
        vec!["oauth".to_string()],
        "permanent quota exhaustion must not inject API key fallback"
    );
}

#[test]
fn test_no_flash_fallback_stops_retry_after_attempt_2() {
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let execution = ExecutionResult {
        summary: "failed".to_string(),
        output: String::new(),
        stderr_output: "HTTP 429 Too Many Requests".to_string(),
        exit_code: 1,
        peak_memory_mb: None,
    };
    let mut env = HashMap::new();
    env.insert("_CSA_NO_FLASH_FALLBACK".to_string(), "1".to_string());
    assert!(
        transport
            .should_retry_gemini_rate_limited(&execution, 1, Some(&env))
            .is_some()
    );
    assert!(
        transport
            .should_retry_gemini_rate_limited(&execution, 2, Some(&env))
            .is_none()
    );
    assert!(
        transport
            .should_retry_gemini_rate_limited(&execution, 2, None)
            .is_some()
    );
}

#[test]
fn test_no_failover_stops_retry_after_attempt_1() {
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let execution = ExecutionResult {
        summary: "failed".to_string(),
        output: String::new(),
        stderr_output: "HTTP 429 Too Many Requests".to_string(),
        exit_code: 1,
        peak_memory_mb: None,
    };
    let mut env = HashMap::new();
    env.insert("_CSA_NO_FAILOVER".to_string(), "1".to_string());

    assert!(
        transport
            .should_retry_gemini_rate_limited(&execution, 1, Some(&env))
            .is_none()
    );
}
