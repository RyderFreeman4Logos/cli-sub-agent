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
    assert!(result.is_some());
    assert_eq!(result.unwrap().matched_pattern, "quota exceeded");
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
    assert_eq!(detected.matched_pattern, "quota_exhausted");
    assert_eq!(detected.reason, "QUOTA_EXHAUSTED");
    assert!(detected.advance_to_next_model);
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
    assert_eq!(detected.matched_pattern, "429_quota_exhausted");
    assert_eq!(detected.reason, "429_quota_exhausted");
    assert!(detected.advance_to_next_model);
    assert_eq!(
        detected.model_spec.as_deref(),
        Some("codex/openai/gpt-5.4/high")
    );
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
    assert_eq!(detected.reason, "Invalid API key");
    assert!(detected.advance_to_next_model);
}

#[test]
fn test_http_403_advances_to_next_model() {
    let detected =
        detect_rate_limit("codex", "HTTP 403 Forbidden", "", 1, None).expect("403 should classify");
    assert_eq!(detected.reason, "HTTP 403");
    assert!(detected.advance_to_next_model);
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
        ("gemini-cli", "quota_exhausted flag is set"),
        ("gemini-cli", "daily quota limit reached"),
        ("gemini-cli", "monthly spending cap reached"),
        ("gemini-cli", "quota exceeded for account"),
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
        ("gemini-cli", "resource exhausted: no more capacity"),
        ("gemini-cli", "capacity_exhausted for model"),
        ("gemini-cli", "HTTP 401 Unauthorized"),
        ("gemini-cli", "HTTP 403 Forbidden"),
        ("codex", "rate_limit_exceeded for key"),
        ("codex", "RateLimitError: please wait"),
        ("codex", "HTTP 429"),
        ("claude-code", "rate limit hit, retry in 60s"),
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
