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
}
