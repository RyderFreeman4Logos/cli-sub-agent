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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
fn test_gemini_permanent_quota_not_triggered_by_reviewed_content_in_stdout() {
    // #1736 regression: a `csa review` whose reviewed diff/source/commit message
    // literally contains a quota phrase (e.g. this failover classifier's own
    // source) must NOT be misclassified as provider quota exhaustion. The marker
    // here lives only in agent stdout (`output`) and the stdout-derived
    // `summary`, never in the provider error channel (`stderr_output`).
    let execution = ExecutionResult {
        summary: "+    pattern: \"monthly spending cap\",".to_string(),
        output: "Reviewing diff:\n+const PATTERN: &str = \"monthly spending cap\";\n\
                  +// matches QUOTA_EXHAUSTED / RESOURCE_EXHAUSTED markers\n\
                  Verdict: PASS"
            .to_string(),
        stderr_output: String::new(),
        exit_code: 1,
        peak_memory_mb: None,
        ..Default::default()
    };
    assert_eq!(
        detect_gemini_permanent_quota_exhaustion_result(&execution),
        None,
        "reviewed quota phrase in stdout/summary must not be read as provider quota exhaustion"
    );
}

#[test]
fn test_gemini_permanent_quota_still_detected_from_provider_stderr() {
    // Positive: a genuine provider quota error on the stderr channel (as the
    // gemini-cli backend emits, mirrored by the fake-gemini harness writing the
    // failure reason with `>&2`) MUST still be detected as permanent.
    let execution = ExecutionResult {
        summary: "Verdict: PASS".to_string(),
        output: "Reviewing diff... Verdict: PASS".to_string(),
        stderr_output:
            "reason: 'RESOURCE_EXHAUSTED'; the monthly spending cap was reached for this project"
                .to_string(),
        exit_code: 1,
        peak_memory_mb: None,
        ..Default::default()
    };
    assert_eq!(
        detect_gemini_permanent_quota_exhaustion_result(&execution),
        Some("monthly spending cap"),
        "provider-reported monthly spending cap on stderr must still be permanent quota"
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
        ..Default::default()
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
fn test_should_retry_codex_transient_429_until_retry_budget_exhausted() {
    let transport = LegacyTransport::new(Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::CodexRuntimeMetadata::from_transport(
            crate::codex_runtime::CodexTransport::Cli,
        ),
    });
    // Since #1460, rate-limit detection is stderr-only; put the pattern in stderr.
    let execution = ExecutionResult {
        summary: "429_quota_exhausted: repeated 3 identical 429/quota errors: HTTP 429 Too Many Requests; Retry-After: 30".to_string(),
        output: String::new(),
        stderr_output:
            "HTTP 429 Too Many Requests; Retry-After: 30".to_string(),
        exit_code: 1,
        peak_memory_mb: None,
        ..Default::default()
    };

    assert_eq!(
        transport
            .should_retry_codex_rate_limited(&execution, 0, None)
            .map(|duration| duration.as_secs()),
        Some(30)
    );
    assert_eq!(
        transport
            .should_retry_codex_rate_limited(&execution, 1, None)
            .map(|duration| duration.as_secs()),
        Some(60)
    );
    assert_eq!(
        transport
            .should_retry_codex_rate_limited(&execution, 2, None)
            .map(|duration| duration.as_secs()),
        Some(120)
    );
    assert!(
        transport
            .should_retry_codex_rate_limited(&execution, 3, None)
            .is_none()
    );
}

#[test]
fn test_should_not_retry_codex_explicit_usage_limit() {
    let transport = LegacyTransport::new(Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::CodexRuntimeMetadata::from_transport(
            crate::codex_runtime::CodexTransport::Cli,
        ),
    });
    // Since #1460, quota detection is stderr-only; put the pattern in stderr.
    let execution = ExecutionResult {
        summary: r#"Internal error: {"codex_error_info":"usage_limit_exceeded"}"#.to_string(),
        output: String::new(),
        stderr_output: r#"Internal error: {"codex_error_info":"usage_limit_exceeded"}"#
            .to_string(),
        exit_code: 1,
        peak_memory_mb: None,
        ..Default::default()
    };

    assert!(
        transport
            .should_retry_codex_rate_limited(&execution, 0, None)
            .is_none()
    );
    assert!(is_codex_permanent_quota_result(&execution));
}

#[test]
fn test_codex_retry_exhausted_summary_marks_permanent_after_backoff_budget() {
    let mut execution = ExecutionResult {
        summary: "HTTP 429 Too Many Requests".to_string(),
        output: String::new(),
        stderr_output: String::new(),
        exit_code: 1,
        peak_memory_mb: None,
        ..Default::default()
    };

    apply_codex_rate_limit_retry_exhausted_summary(&mut execution, CODEX_RATE_LIMIT_MAX_RETRIES);

    assert!(execution.summary.starts_with("codex_429_retry_exhausted"));
    assert!(is_codex_permanent_quota_result(&execution));
    assert!(
        !is_codex_transient_rate_limit_result(&execution),
        "retry-exhausted marker must stop another same-tool retry loop"
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
        ..Default::default()
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
        ..Default::default()
    };
    let mut env = HashMap::new();
    env.insert("_CSA_NO_FAILOVER".to_string(), "1".to_string());

    assert!(
        transport
            .should_retry_gemini_rate_limited(&execution, 1, Some(&env))
            .is_none()
    );
}
