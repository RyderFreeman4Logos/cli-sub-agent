use super::*;

#[test]
fn test_gemini_resource_exhausted() {
    let result = detect_rate_limit(
        "gemini-cli",
        "Error: Resource exhausted. Please try again later.",
        "",
        1,
        None,
    );
    assert!(result.is_some());
    let detected = result.unwrap();
    assert_eq!(detected.matched_pattern, "resource exhausted");
    assert!(
        !detected.quota_exhausted,
        "plain RESOURCE_EXHAUSTED can be transient and must not imply permanent quota"
    );
}

#[test]
fn test_codex_rate_limit() {
    let result = detect_rate_limit(
        "codex",
        "",
        "Error: rate_limit_exceeded - Too many requests",
        1,
        None,
    );
    assert!(result.is_some());
}

#[test]
fn test_claude_overloaded() {
    let result = detect_rate_limit("claude-code", "API overloaded, please retry", "", 1, None);
    assert!(result.is_some());
}

#[test]
fn test_no_rate_limit_on_success() {
    let result = detect_rate_limit("gemini-cli", "Some output with 429 in it", "", 0, None);
    assert!(
        result.is_none(),
        "Should not detect rate limit on exit_code=0"
    );
}

#[test]
fn test_no_rate_limit_unrelated_error() {
    let result = detect_rate_limit("codex", "Syntax error in prompt", "", 1, None);
    assert!(result.is_none());
}

#[test]
fn test_generic_429_pattern() {
    let result = detect_rate_limit("unknown-tool", "HTTP 429 Too Many Requests", "", 2, None);
    assert!(result.is_some());
}

#[test]
fn test_gemini_quota_exceeded() {
    let result = detect_rate_limit(
        "gemini-cli",
        "API error: quota exceeded for model gemini-2.5-pro",
        "",
        1,
        None,
    );
    let detected = result.expect("quota exceeded should classify");
    assert_eq!(detected.matched_pattern, "rate-limit-429");
    assert_eq!(detected.reason, "HTTP 429");
    assert!(detected.advance_to_next_model);
    assert!(!detected.quota_exhausted);
}

#[test]
fn test_gemini_oauth_browser_prompt_is_auth_unavailable_failover() {
    let result = detect_rate_limit(
        "gemini-cli",
        "Opening authentication page in your browser. Do you want to continue? [Y/n]:",
        "",
        1,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
    );

    let detected = result.expect("OAuth browser prompt should classify");
    assert_eq!(detected.reason, "auth_unavailable");
    assert_eq!(detected.matched_pattern, "auth_unavailable");
    assert!(detected.advance_to_next_model);
    assert!(!detected.quota_exhausted);
    assert_eq!(
        detected.model_spec.as_deref(),
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh")
    );
}

#[test]
fn issue_1719_quota_and_auth_failover_are_transient_and_sanitized() {
    let cases = [
        ("reason: QUOTA_EXHAUSTED", "rate-limit-429", "HTTP 429"),
        (
            r#"Error when talking to Gemini API: _ApiError: {"error":{"message":"API Key not found","code":400,"status":"INVALID_ARGUMENT"}}"#,
            "auth_unavailable",
            "auth_unavailable",
        ),
    ];

    for (stderr, expected_marker, expected_reason) in cases {
        let detected = detect_rate_limit(
            "gemini-cli",
            stderr,
            "",
            1,
            Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        )
        .unwrap_or_else(|| panic!("expected transient failover classification for {stderr}"));

        assert_eq!(detected.matched_pattern, expected_marker);
        assert_eq!(detected.reason, expected_reason);
        assert!(detected.advance_to_next_model);
        assert!(!detected.quota_exhausted);
        assert!(!requires_init_failure_window(&detected));
    }
}

#[test]
fn test_gemini_resource_exhausted_uppercase() {
    let result = detect_rate_limit(
        "gemini-cli",
        "google.api.error: RESOURCE_EXHAUSTED",
        "",
        1,
        None,
    );
    assert!(result.is_some());
    let detected = result.unwrap();
    assert_eq!(detected.matched_pattern, "resource_exhausted");
    assert!(
        !detected.quota_exhausted,
        "RESOURCE_EXHAUSTED status alone is not permanent quota exhaustion"
    );
}

#[test]
fn test_codex_rate_limit_error_type() {
    let result = detect_rate_limit("codex", "", "RateLimitError: please wait", 1, None);
    assert!(result.is_some());
    assert_eq!(result.unwrap().matched_pattern, "ratelimiterror");
}

#[test]
fn test_claude_529_overloaded() {
    let result = detect_rate_limit("claude-code", "HTTP 529 Service Overloaded", "", 1, None);
    assert!(result.is_some());
    let detected = result.unwrap();
    assert_eq!(detected.matched_pattern, "529");
    assert!(!detected.advance_to_next_model);
}

#[test]
fn test_opencode_too_many_requests() {
    let result = detect_rate_limit(
        "opencode",
        "Error: too many requests, please slow down",
        "",
        1,
        None,
    );
    assert!(result.is_some());
    assert_eq!(result.unwrap().matched_pattern, "too many requests");
}

#[test]
fn test_opencode_rate_limit_case_sensitive() {
    let result = detect_rate_limit("opencode", "Rate limit exceeded for account", "", 1, None);
    assert!(result.is_some());
    assert_eq!(result.unwrap().matched_pattern, "rate limit");
}

#[test]
fn test_no_match_for_unknown_error_string() {
    let result = detect_rate_limit(
        "gemini-cli",
        "Error: invalid prompt format",
        "No valid model found",
        1,
        None,
    );
    assert!(
        result.is_none(),
        "Should not detect rate limit for unrelated error"
    );
}

#[test]
fn test_pattern_match_in_stdout_not_stderr() {
    let result = detect_rate_limit("codex", "", "rate_limit_exceeded", 1, None);
    assert!(result.is_some());
}

#[test]
fn test_combined_stderr_stdout_checked() {
    let result = detect_rate_limit("codex", "rate_limit", "_exceeded in output", 1, None);
    assert!(
        result.is_none(),
        "Split pattern across stderr/stdout should not match"
    );
}

#[test]
fn test_codex_usage_limit_exceeded() {
    let result = detect_rate_limit(
        "codex",
        r#"Internal error: {"codex_error_info": "usage_limit_exceeded", "message": "You've hit your usage limit."}"#,
        "",
        1,
        None,
    );
    assert!(result.is_some());
    assert_eq!(result.unwrap().matched_pattern, "usage_limit_exceeded");
}

#[test]
fn test_codex_usage_limit_plain_text() {
    let result = detect_rate_limit(
        "codex",
        "Error: Usage limit exceeded for this account",
        "",
        1,
        None,
    );
    assert!(result.is_some());
    assert_eq!(result.unwrap().matched_pattern, "usage limit");
}

#[test]
fn test_empty_stderr_and_stdout() {
    let result = detect_rate_limit("gemini-cli", "", "", 1, None);
    assert!(result.is_none());
}

#[test]
fn test_tool_name_preserved_in_result() {
    let result = detect_rate_limit("claude-code", "rate limit hit", "", 1, None);
    assert!(result.is_some());
    let detected = result.unwrap();
    assert_eq!(detected.tool, "claude-code");
    assert_eq!(detected.matched_pattern, "rate limit");
    assert_eq!(detected.reason, "HTTP 429");
}

#[test]
fn test_first_matching_pattern_wins() {
    let result = detect_rate_limit("gemini-cli", "Resource exhausted (429)", "", 1, None);
    assert!(result.is_some());
    assert_eq!(result.unwrap().matched_pattern, "429");
}

#[test]
fn test_model_spec_preserved_in_result() {
    let result = detect_rate_limit(
        "gemini-cli",
        "Resource exhausted",
        "",
        1,
        Some("gemini-cli/google/gemini-2.5-pro/high"),
    );
    assert!(result.is_some());
    let detected = result.unwrap();
    assert_eq!(
        detected.model_spec.as_deref(),
        Some("gemini-cli/google/gemini-2.5-pro/high")
    );
}

#[test]
fn test_gemini_model_capacity_exhausted() {
    let result = detect_rate_limit(
        "gemini-cli",
        "429 MODEL_CAPACITY_EXHAUSTED: No capacity available for model gemini-3-flash-preview",
        "",
        1,
        Some("gemini-cli/google/gemini-3-flash-preview/xhigh"),
    );
    assert!(result.is_some());
    // Should match "429" first (patterns are checked in order)
    let detected = result.unwrap();
    assert_eq!(detected.matched_pattern, "429");
    assert_eq!(
        detected.model_spec.as_deref(),
        Some("gemini-cli/google/gemini-3-flash-preview/xhigh")
    );
}

#[test]
fn test_gemini_capacity_exhausted_without_429_prefix() {
    let result = detect_rate_limit(
        "gemini-cli",
        "Error: capacity_exhausted for model",
        "",
        1,
        None,
    );
    assert!(result.is_some());
    assert_eq!(result.unwrap().matched_pattern, "capacity_exhausted");
}

#[test]
fn test_gemini_quota_exhausted_case_insensitive() {
    let result = detect_rate_limit("gemini-cli", "reason: 'QUOTA_EXHAUSTED'", "", 1, None);
    assert!(result.is_some());
    let detected = result.unwrap();
    assert_eq!(detected.matched_pattern, "rate-limit-429");
    assert_eq!(detected.reason, "HTTP 429");
    assert!(detected.advance_to_next_model);
    assert!(!detected.quota_exhausted);
}

#[test]
fn test_persistent_429_quota_exhausted_advances_to_next_model() {
    let detected = detect_rate_limit(
        "codex",
        "429_quota_exhausted: repeated 3 identical 429/quota errors: Error: exceeded its quota",
        "",
        1,
        Some("codex/openai/gpt-5.4/high"),
    )
    .expect("persistent 429 summary should classify");
    assert_eq!(detected.matched_pattern, "rate-limit-429");
    assert_eq!(detected.reason, "HTTP 429");
    assert!(detected.advance_to_next_model);
    assert!(
        !detected.quota_exhausted,
        "codex 429_quota_exhausted is a transient retry signal until executor retries are exhausted"
    );
    assert_eq!(
        detected.model_spec.as_deref(),
        Some("codex/openai/gpt-5.4/high")
    );
}

#[test]
fn test_codex_429_retry_exhausted_is_permanent_after_backoff_retries() {
    let detected = detect_rate_limit(
        "codex",
        "codex_429_retry_exhausted: temporary codex 429 rate limit persisted after 3 retries",
        "",
        1,
        Some("codex/openai/gpt-5.4/high"),
    )
    .expect("codex retry exhaustion should classify");
    assert_eq!(detected.matched_pattern, "codex_429_retry_exhausted");
    assert_eq!(detected.reason, "codex_429_retry_exhausted");
    assert!(detected.advance_to_next_model);
    assert!(detected.quota_exhausted);
    assert_eq!(
        detected.model_spec.as_deref(),
        Some("codex/openai/gpt-5.4/high")
    );
}

#[test]
fn test_codex_429_with_billing_quota_is_permanent_without_retry_budget() {
    let detected = detect_rate_limit(
        "codex",
        "429_quota_exhausted: monthly spending cap reached for billing account",
        "",
        1,
        None,
    )
    .expect("billing quota 429 should classify");
    assert_eq!(detected.matched_pattern, "monthly spending cap");
    assert!(detected.quota_exhausted);
}

#[test]
fn test_gemini_invalid_api_key_advances_to_next_model() {
    let detected = detect_rate_limit(
        "gemini-cli",
        "_ApiError: {\"error\":\"Invalid API key\"}",
        "",
        1,
        None,
    )
    .expect("invalid api key should classify");
    assert_eq!(detected.matched_pattern, "auth_unavailable");
    assert_eq!(detected.reason, "auth_unavailable");
    assert!(detected.advance_to_next_model);
    assert!(!detected.quota_exhausted);
}

#[test]
fn test_gemini_api_key_not_found_json_400_advances_to_next_model() {
    let detected = detect_rate_limit(
        "gemini-cli",
        r#"Error when talking to Gemini API: _ApiError: {"error":{"message":"API Key not found","code":400,"status":"INVALID_ARGUMENT"}}"#,
        "",
        1,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
    )
    .expect("Gemini API key-not-found 400 should classify");
    assert_eq!(detected.matched_pattern, "auth_unavailable");
    assert_eq!(detected.reason, "auth_unavailable");
    assert!(detected.advance_to_next_model);
    assert!(!detected.quota_exhausted);
    assert_eq!(
        detected.model_spec.as_deref(),
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh")
    );
}

#[test]
fn test_http_403_advances_to_next_model() {
    let detected =
        detect_rate_limit("codex", "HTTP 403 Forbidden", "", 1, None).expect("403 should classify");
    assert_eq!(detected.reason, "HTTP 403");
    assert!(detected.advance_to_next_model);
}

#[test]
fn test_gemini_http_400_advances_to_next_model() {
    let detected = detect_rate_limit(
        "gemini-cli",
        "Error: request failed with status: 400 Bad Request",
        "",
        1,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
    )
    .expect("HTTP 400 should classify");
    assert_eq!(detected.reason, "HTTP 400");
    assert!(detected.advance_to_next_model);
    assert!(requires_init_failure_window(&detected));
    assert_eq!(
        detected.model_spec.as_deref(),
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh")
    );
}

#[test]
fn test_http_5xx_status_patterns_advance_to_next_model() {
    for (message, expected_reason) in [
        ("HTTP 500 Internal Server Error", "HTTP 500"),
        ("upstream returned status 502", "HTTP 502"),
        ("server unavailable; status: 503", "HTTP 503"),
        ("provider returned status: 5xx", "HTTP 5xx"),
    ] {
        let detected = detect_rate_limit("gemini-cli", message, "", 1, None)
            .unwrap_or_else(|| panic!("{message} should classify"));
        assert_eq!(detected.reason, expected_reason);
        assert!(detected.advance_to_next_model);
        assert!(requires_init_failure_window(&detected));
    }
}

#[test]
fn test_claude_crash_retry_exhausted() {
    let result = detect_rate_limit(
        "claude-code",
        "ACP transport failed: crash retry exhausted after repeated server shutdowns",
        "",
        1,
        None,
    );
    assert!(result.is_some());
    assert_eq!(result.unwrap().matched_pattern, "crash retry exhausted");
}

#[test]
fn test_gemini_retry_chain_exhausted() {
    let result = detect_rate_limit(
        "gemini-cli",
        "Retry chain exhausted after OAuth -> API key -> flash fallback",
        "",
        1,
        None,
    );
    assert!(result.is_some());
    let detected = result.unwrap();
    assert_eq!(detected.matched_pattern, "retry chain exhausted");
    assert!(
        detected.advance_to_next_model,
        "gemini retry chain exhaustion must trigger tier-level failover to next tool"
    );
}

#[test]
fn test_gemini_retry_chain_exhausted_triggers_tier_failover() {
    let result = detect_rate_limit(
        "gemini-cli",
        "gemini acp retry chain exhausted after all fallback phases failed",
        "",
        1,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
    );
    assert!(result.is_some());
    let detected = result.unwrap();
    assert!(
        detected.advance_to_next_model,
        "tier failover must activate when gemini retry chain is exhausted (#1320)"
    );
    assert_eq!(detected.reason, "gemini_retry_chain_exhausted");
    assert_eq!(
        detected.model_spec.as_deref(),
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh")
    );
}

// --- quota_exhausted flag tests (#1346) ---

#[test]
fn test_quota_exhausted_true_for_permanent_quota_patterns() {
    let quota_patterns = [
        ("gemini-cli", "daily quota limit reached"),
        ("gemini-cli", "monthly spending cap reached"),
        ("codex", "usage_limit_exceeded for this period"),
        ("codex", "Error: usage limit exceeded"),
        ("codex", "daily quota exhausted"),
    ];
    for (tool, msg) in quota_patterns {
        let result = detect_rate_limit(tool, msg, "", 1, None);
        let detected = result.unwrap_or_else(|| panic!("no match for '{msg}'"));
        assert!(
            detected.quota_exhausted,
            "quota_exhausted must be true for '{msg}' (tool={tool})"
        );
    }
}

#[test]
fn test_quota_exhausted_false_for_transient_rate_limits() {
    let transient_patterns = [
        ("gemini-cli", "HTTP 429 Too Many Requests"),
        ("gemini-cli", "reason: QUOTA_EXHAUSTED"),
        ("gemini-cli", "quota_exhausted flag is set"),
        ("gemini-cli", "quota exceeded for account"),
        ("gemini-cli", "api key not found"),
        ("gemini-cli", "invalid api key"),
        ("gemini-cli", "resource exhausted: no more capacity"),
        ("gemini-cli", "capacity_exhausted for model"),
        ("gemini-cli", "HTTP 401 Unauthorized"),
        ("gemini-cli", "HTTP 403 Forbidden"),
        ("codex", "rate_limit_exceeded for key"),
        ("codex", "RateLimitError: please wait"),
        ("codex", "HTTP 429"),
        ("codex", "429_quota_exhausted: repeated transient 429"),
        ("opencode", "429_quota_exhausted: repeated transient 429"),
        ("claude-code", "rate limit hit, retry in 60s"),
        ("claude-code", "429_quota_exhausted: repeated transient 429"),
        ("claude-code", "HTTP 529 Service Overloaded"),
        ("claude-code", "API overloaded, please retry"),
    ];
    for (tool, msg) in transient_patterns {
        let result = detect_rate_limit(tool, msg, "", 1, None);
        let detected = result.unwrap_or_else(|| panic!("no match for '{msg}'"));
        assert!(
            !detected.quota_exhausted,
            "quota_exhausted must be false for transient pattern '{msg}' (tool={tool})"
        );
    }
}

#[test]
fn test_quota_exhausted_patterns_do_not_set_quota_from_stdout() {
    let stdout_only_quota_patterns = [
        ("gemini-cli", "reason: QUOTA_EXHAUSTED"),
        (
            "codex",
            "render_billing_disabled_banner monthly spending cap",
        ),
        ("claude-code", "429_quota_exhausted"),
        ("opencode", "429_quota_exhausted"),
        ("unknown-tool", "429_quota_exhausted"),
    ];
    for (tool, stdout) in stdout_only_quota_patterns {
        let result = detect_rate_limit(tool, "process exited with status 1", stdout, 1, None);
        if let Some(detected) = result {
            assert!(
                !detected.quota_exhausted,
                "stdout-only quota pattern must not set quota_exhausted (tool={tool}, stdout={stdout})"
            );
        }
    }
}

// --- #1473: codex quota error in stdout JSON triggers failover ---

#[test]
fn test_codex_usage_limit_in_stdout_json_triggers_failover() {
    let detected = detect_rate_limit(
        "codex",
        "",
        r#"{"type":"error","message":"You've hit your usage limit for GPT-5.3-Codex-Spark. Switch to another model now, or try again at 3:50 AM."}"#,
        1,
        Some("codex/openai/gpt-5.3-codex-spark/xhigh"),
    )
    .expect("codex stdout usage limit should classify for failover");
    assert_eq!(detected.matched_pattern, "usage limit");
    assert!(
        detected.advance_to_next_model,
        "must advance to next tier model on codex quota hit"
    );
    assert!(
        !detected.quota_exhausted,
        "stdout-only match should not set permanent quota flag"
    );
    assert_eq!(
        detected.model_spec.as_deref(),
        Some("codex/openai/gpt-5.3-codex-spark/xhigh")
    );
}

#[test]
fn test_codex_usage_limit_in_stderr_sets_quota_exhausted() {
    let detected = detect_rate_limit(
        "codex",
        "Error: Usage limit exceeded for this account",
        "",
        1,
        None,
    )
    .expect("codex stderr usage limit should classify");
    assert!(
        detected.quota_exhausted,
        "stderr-confirmed match should set permanent quota flag"
    );
}

#[test]
fn test_transient_rate_limit_patterns_still_match_stdout() {
    let detected = detect_rate_limit(
        "codex",
        "process exited with status 1",
        "HTTP 429 Too Many Requests",
        1,
        None,
    )
    .expect("transient stdout rate limit should still classify");

    assert_eq!(detected.matched_pattern, "429");
    assert_eq!(detected.reason, "HTTP 429");
    assert!(!detected.quota_exhausted);
}

#[test]
fn test_gemini_retry_chain_exhausted_is_not_permanent_quota() {
    let result = detect_rate_limit(
        "gemini-cli",
        "retry chain exhausted after all OAuth and API key fallbacks failed",
        "",
        1,
        None,
    );
    let detected = result.expect("retry chain exhausted should be detected");
    assert!(
        !detected.quota_exhausted,
        "gemini retry chain exhaustion is not permanent quota exhaustion by itself"
    );
}

#[test]
fn test_acp_crash_retry_exhausted_is_not_quota_exhausted() {
    let result = detect_rate_limit(
        "claude-code",
        "crash retry exhausted after repeated server shutdowns",
        "",
        1,
        None,
    );
    let detected = result.expect("acp crash retry exhausted should be detected");
    assert!(
        !detected.quota_exhausted,
        "ACP crash retry exhaustion is a process crash, not quota exhaustion (#1346)"
    );
}
