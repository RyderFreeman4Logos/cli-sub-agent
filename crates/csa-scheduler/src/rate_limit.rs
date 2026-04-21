//! Classify stderr/stdout conditions that may require tier failover.

use serde::Serialize;

const ACP_CRASH_EXHAUSTION_PATTERNS: &[&str] =
    &["acp crash retry exhausted", "crash retry exhausted"];
const GEMINI_RETRY_CHAIN_EXHAUSTION_PATTERNS: &[&str] =
    &["gemini acp retry chain exhausted", "retry chain exhausted"];

/// Information about a detected rate-limit event.
#[derive(Debug, Clone, Serialize)]
pub struct RateLimitDetected {
    pub tool: String,
    pub matched_pattern: String,
    pub reason: String,
    pub advance_to_next_model: bool,
    /// The model spec that was running when the rate limit hit (e.g.
    /// "gemini-cli/google/gemini-2.5-pro/high"). Enables tier-aware failover
    /// to pick an equivalent model from a different family.
    pub model_spec: Option<String>,
}

#[derive(Clone, Copy)]
struct FailoverPattern {
    pattern: &'static str,
    reason: &'static str,
    advance_to_next_model: bool,
}

const GEMINI_RETRY_CHAIN_FAILOVER_PATTERNS: &[FailoverPattern] = &[
    FailoverPattern {
        pattern: GEMINI_RETRY_CHAIN_EXHAUSTION_PATTERNS[0],
        reason: "gemini_retry_chain_exhausted",
        advance_to_next_model: false,
    },
    FailoverPattern {
        pattern: GEMINI_RETRY_CHAIN_EXHAUSTION_PATTERNS[1],
        reason: "gemini_retry_chain_exhausted",
        advance_to_next_model: false,
    },
];

const ACP_CRASH_FAILOVER_PATTERNS: &[FailoverPattern] = &[
    FailoverPattern {
        pattern: ACP_CRASH_EXHAUSTION_PATTERNS[0],
        reason: "acp_crash_retry_exhausted",
        advance_to_next_model: false,
    },
    FailoverPattern {
        pattern: ACP_CRASH_EXHAUSTION_PATTERNS[1],
        reason: "acp_crash_retry_exhausted",
        advance_to_next_model: false,
    },
];

/// Check stderr/stdout for failover indicators.
///
/// Each tool emits different error messages for quota/auth/permission failures.
/// This function normalizes the known patterns and indicates whether the caller
/// should advance to the next tier model.
pub fn detect_rate_limit(
    tool_name: &str,
    stderr: &str,
    stdout: &str,
    exit_code: i32,
    model_spec: Option<&str>,
) -> Option<RateLimitDetected> {
    // Non-zero exit + pattern match
    if exit_code == 0 {
        return None;
    }

    let combined_lower = format!("{stderr}\n{stdout}").to_ascii_lowercase();
    for pattern in patterns_for_tool(tool_name)
        .iter()
        .chain(failover_patterns_for_tool(tool_name).iter())
    {
        if combined_lower.contains(pattern.pattern) {
            return Some(RateLimitDetected {
                tool: tool_name.to_string(),
                matched_pattern: pattern.pattern.to_string(),
                reason: pattern.reason.to_string(),
                advance_to_next_model: pattern.advance_to_next_model,
                model_spec: model_spec.map(String::from),
            });
        }
    }

    None
}

fn failover_patterns_for_tool(tool: &str) -> &'static [FailoverPattern] {
    match tool {
        "gemini-cli" => GEMINI_RETRY_CHAIN_FAILOVER_PATTERNS,
        "codex" | "claude-code" => ACP_CRASH_FAILOVER_PATTERNS,
        _ => &[],
    }
}

fn patterns_for_tool(tool: &str) -> &'static [FailoverPattern] {
    match tool {
        "gemini-cli" => &[
            FailoverPattern {
                pattern: "429",
                reason: "HTTP 429",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "quota_exhausted",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "quota exhausted",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "resource exhausted",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "resource_exhausted",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "capacity exhausted",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "capacity_exhausted",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "exhausted your capacity",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "no capacity available",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "quota exceeded",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "http 401",
                reason: "HTTP 401",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "_apierror: {\"error\":\"invalid api key\"}",
                reason: "Invalid API key",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "invalid api key",
                reason: "Invalid API key",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "http 403",
                reason: "HTTP 403",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "forbidden",
                reason: "HTTP 403",
                advance_to_next_model: true,
            },
        ],
        "opencode" => &[
            FailoverPattern {
                pattern: "rate limit",
                reason: "HTTP 429",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "429",
                reason: "HTTP 429",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "too many requests",
                reason: "HTTP 429",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "http 401",
                reason: "HTTP 401",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "http 403",
                reason: "HTTP 403",
                advance_to_next_model: true,
            },
        ],
        "codex" => &[
            FailoverPattern {
                pattern: "rate_limit_exceeded",
                reason: "HTTP 429",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "usage_limit_exceeded",
                reason: "HTTP 429",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "usage limit",
                reason: "HTTP 429",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "ratelimiterror",
                reason: "HTTP 429",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "429",
                reason: "HTTP 429",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "http 401",
                reason: "HTTP 401",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "invalid api key",
                reason: "Invalid API key",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "http 403",
                reason: "HTTP 403",
                advance_to_next_model: true,
            },
        ],
        "claude-code" => &[
            FailoverPattern {
                pattern: "rate limit",
                reason: "HTTP 429",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "429",
                reason: "HTTP 429",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "529",
                reason: "HTTP 529",
                advance_to_next_model: false,
            },
            FailoverPattern {
                pattern: "overloaded",
                reason: "HTTP 529",
                advance_to_next_model: false,
            },
            FailoverPattern {
                pattern: "http 401",
                reason: "HTTP 401",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "http 403",
                reason: "HTTP 403",
                advance_to_next_model: true,
            },
        ],
        _ => &[
            FailoverPattern {
                pattern: "429",
                reason: "HTTP 429",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "rate limit",
                reason: "HTTP 429",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "http 401",
                reason: "HTTP 401",
                advance_to_next_model: true,
            },
            FailoverPattern {
                pattern: "http 403",
                reason: "HTTP 403",
                advance_to_next_model: true,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(result.unwrap().matched_pattern, "resource exhausted");
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
        assert_eq!(result.unwrap().matched_pattern, "resource_exhausted");
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
        let detected = detect_rate_limit("codex", "HTTP 403 Forbidden", "", 1, None)
            .expect("403 should classify");
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
        assert_eq!(result.unwrap().matched_pattern, "retry chain exhausted");
    }
}
