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
    /// True when the failure is due to permanent quota exhaustion (daily/monthly cap),
    /// as opposed to a transient rate limit that may clear with backoff.
    pub quota_exhausted: bool,
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
    /// Whether this pattern indicates permanent quota exhaustion.
    quota_exhausted: bool,
}

const GEMINI_RETRY_CHAIN_FAILOVER_PATTERNS: &[FailoverPattern] = &[
    FailoverPattern {
        pattern: GEMINI_RETRY_CHAIN_EXHAUSTION_PATTERNS[0],
        reason: "gemini_retry_chain_exhausted",
        advance_to_next_model: true,
        quota_exhausted: true,
    },
    FailoverPattern {
        pattern: GEMINI_RETRY_CHAIN_EXHAUSTION_PATTERNS[1],
        reason: "gemini_retry_chain_exhausted",
        advance_to_next_model: true,
        quota_exhausted: true,
    },
];

const ACP_CRASH_FAILOVER_PATTERNS: &[FailoverPattern] = &[
    FailoverPattern {
        pattern: ACP_CRASH_EXHAUSTION_PATTERNS[0],
        reason: "acp_crash_retry_exhausted",
        advance_to_next_model: false,
        quota_exhausted: false,
    },
    FailoverPattern {
        pattern: ACP_CRASH_EXHAUSTION_PATTERNS[1],
        reason: "acp_crash_retry_exhausted",
        advance_to_next_model: false,
        quota_exhausted: false,
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
                quota_exhausted: pattern.quota_exhausted,
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
                pattern: "429_quota_exhausted",
                reason: "429_quota_exhausted",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "429",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "quota_exhausted",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "quota exhausted",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "daily quota",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "resource exhausted",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "resource_exhausted",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "capacity exhausted",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "capacity_exhausted",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "exhausted your capacity",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "no capacity available",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "quota exceeded",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "http 401",
                reason: "HTTP 401",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "_apierror: {\"error\":\"invalid api key\"}",
                reason: "Invalid API key",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "invalid api key",
                reason: "Invalid API key",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "http 403",
                reason: "HTTP 403",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "forbidden",
                reason: "HTTP 403",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
        ],
        "opencode" => &[
            FailoverPattern {
                pattern: "429_quota_exhausted",
                reason: "429_quota_exhausted",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "rate limit",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "429",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "too many requests",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "http 401",
                reason: "HTTP 401",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "http 403",
                reason: "HTTP 403",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
        ],
        "codex" => &[
            FailoverPattern {
                pattern: "429_quota_exhausted",
                reason: "429_quota_exhausted",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "rate_limit_exceeded",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "usage_limit_exceeded",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "usage limit",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "daily quota",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "ratelimiterror",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "429",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "http 401",
                reason: "HTTP 401",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "invalid api key",
                reason: "Invalid API key",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "http 403",
                reason: "HTTP 403",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
        ],
        "claude-code" => &[
            FailoverPattern {
                pattern: "429_quota_exhausted",
                reason: "429_quota_exhausted",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "rate limit",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "429",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "529",
                reason: "HTTP 529",
                advance_to_next_model: false,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "overloaded",
                reason: "HTTP 529",
                advance_to_next_model: false,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "http 401",
                reason: "HTTP 401",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "http 403",
                reason: "HTTP 403",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
        ],
        _ => &[
            FailoverPattern {
                pattern: "429_quota_exhausted",
                reason: "429_quota_exhausted",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "429",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "rate limit",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "http 401",
                reason: "HTTP 401",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "http 403",
                reason: "HTTP 403",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
        ],
    }
}

#[cfg(test)]
#[path = "rate_limit_tests.rs"]
mod tests;
