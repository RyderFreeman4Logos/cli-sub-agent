//! 429 / rate-limit detection from tool stderr and stdout.

use serde::Serialize;

/// Information about a detected rate-limit event.
#[derive(Debug, Clone, Serialize)]
pub struct RateLimitDetected {
    pub tool: String,
    pub matched_pattern: String,
}

/// Check stderr/stdout for rate-limit indicators.
///
/// Each tool emits different error messages when rate-limited. This function
/// checks known patterns and returns `Some` if a match is found.
pub fn detect_rate_limit(
    tool_name: &str,
    stderr: &str,
    stdout: &str,
    exit_code: i32,
) -> Option<RateLimitDetected> {
    // Non-zero exit + pattern match
    if exit_code == 0 {
        return None;
    }

    let combined = format!("{}\n{}", stderr, stdout);
    let patterns = patterns_for_tool(tool_name);

    for pattern in patterns {
        if combined.contains(pattern) {
            return Some(RateLimitDetected {
                tool: tool_name.to_string(),
                matched_pattern: pattern.to_string(),
            });
        }
    }

    None
}

fn patterns_for_tool(tool: &str) -> &'static [&'static str] {
    match tool {
        "gemini-cli" => &[
            "Resource exhausted",
            "429",
            "quota exceeded",
            "RESOURCE_EXHAUSTED",
        ],
        "opencode" => &["rate limit", "429", "too many requests", "Rate limit"],
        "codex" => &["rate_limit_exceeded", "429", "RateLimitError"],
        "claude-code" => &["rate limit", "429", "overloaded", "529"],
        _ => &["429", "rate limit"],
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
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().matched_pattern, "Resource exhausted");
    }

    #[test]
    fn test_codex_rate_limit() {
        let result = detect_rate_limit(
            "codex",
            "",
            "Error: rate_limit_exceeded - Too many requests",
            1,
        );
        assert!(result.is_some());
    }

    #[test]
    fn test_claude_overloaded() {
        let result = detect_rate_limit("claude-code", "API overloaded, please retry", "", 1);
        assert!(result.is_some());
    }

    #[test]
    fn test_no_rate_limit_on_success() {
        let result = detect_rate_limit("gemini-cli", "Some output with 429 in it", "", 0);
        assert!(
            result.is_none(),
            "Should not detect rate limit on exit_code=0"
        );
    }

    #[test]
    fn test_no_rate_limit_unrelated_error() {
        let result = detect_rate_limit("codex", "Syntax error in prompt", "", 1);
        assert!(result.is_none());
    }

    #[test]
    fn test_generic_429_pattern() {
        let result = detect_rate_limit("unknown-tool", "HTTP 429 Too Many Requests", "", 2);
        assert!(result.is_some());
    }

    #[test]
    fn test_gemini_quota_exceeded() {
        let result = detect_rate_limit(
            "gemini-cli",
            "API error: quota exceeded for model gemini-2.5-pro",
            "",
            1,
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().matched_pattern, "quota exceeded");
    }

    #[test]
    fn test_gemini_resource_exhausted_uppercase() {
        let result = detect_rate_limit("gemini-cli", "google.api.error: RESOURCE_EXHAUSTED", "", 1);
        assert!(result.is_some());
        assert_eq!(result.unwrap().matched_pattern, "RESOURCE_EXHAUSTED");
    }

    #[test]
    fn test_codex_rate_limit_error_type() {
        let result = detect_rate_limit("codex", "", "RateLimitError: please wait", 1);
        assert!(result.is_some());
        assert_eq!(result.unwrap().matched_pattern, "RateLimitError");
    }

    #[test]
    fn test_claude_529_overloaded() {
        let result = detect_rate_limit("claude-code", "HTTP 529 Service Overloaded", "", 1);
        assert!(result.is_some());
        assert_eq!(result.unwrap().matched_pattern, "529");
    }

    #[test]
    fn test_opencode_too_many_requests() {
        let result = detect_rate_limit(
            "opencode",
            "Error: too many requests, please slow down",
            "",
            1,
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().matched_pattern, "too many requests");
    }

    #[test]
    fn test_opencode_rate_limit_case_sensitive() {
        // "Rate limit" with capital R is a separate pattern for opencode
        let result = detect_rate_limit("opencode", "Rate limit exceeded for account", "", 1);
        assert!(result.is_some());
        assert_eq!(result.unwrap().matched_pattern, "Rate limit");
    }

    #[test]
    fn test_no_match_for_unknown_error_string() {
        let result = detect_rate_limit(
            "gemini-cli",
            "Error: invalid prompt format",
            "No valid model found",
            1,
        );
        assert!(
            result.is_none(),
            "Should not detect rate limit for unrelated error"
        );
    }

    #[test]
    fn test_pattern_match_in_stdout_not_stderr() {
        // Pattern can appear in stdout alone
        let result = detect_rate_limit("codex", "", "rate_limit_exceeded", 1);
        assert!(result.is_some());
    }

    #[test]
    fn test_combined_stderr_stdout_checked() {
        // Pattern split across stderr/stdout should not match (substring in combined)
        // But if the complete pattern appears in either, it should match
        let result = detect_rate_limit("codex", "rate_limit", "_exceeded in output", 1);
        // "rate_limit" alone does not match "rate_limit_exceeded"
        // but combined = "rate_limit\n_exceeded in output" does not contain "rate_limit_exceeded"
        assert!(
            result.is_none(),
            "Split pattern across stderr/stdout should not match"
        );
    }

    #[test]
    fn test_empty_stderr_and_stdout() {
        let result = detect_rate_limit("gemini-cli", "", "", 1);
        assert!(result.is_none());
    }

    #[test]
    fn test_tool_name_preserved_in_result() {
        let result = detect_rate_limit("claude-code", "rate limit hit", "", 1);
        assert!(result.is_some());
        let detected = result.unwrap();
        assert_eq!(detected.tool, "claude-code");
        assert_eq!(detected.matched_pattern, "rate limit");
    }

    #[test]
    fn test_first_matching_pattern_wins() {
        // gemini-cli patterns: "Resource exhausted", "429", "quota exceeded", "RESOURCE_EXHAUSTED"
        // If multiple match, the first one should be returned
        let result = detect_rate_limit("gemini-cli", "Resource exhausted (429)", "", 1);
        assert!(result.is_some());
        // "Resource exhausted" comes before "429" in the pattern list
        assert_eq!(result.unwrap().matched_pattern, "Resource exhausted");
    }
}
