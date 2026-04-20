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
fn test_should_retry_on_quota_exhausted_marker() {
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
