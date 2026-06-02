//! Classify stderr/stdout conditions that may require tier failover.

use serde::Serialize;
use std::time::Duration;

const ACP_CRASH_EXHAUSTION_PATTERNS: &[&str] =
    &["acp crash retry exhausted", "crash retry exhausted"];
const GEMINI_RETRY_CHAIN_EXHAUSTION_PATTERNS: &[&str] =
    &["gemini acp retry chain exhausted", "retry chain exhausted"];
pub const INIT_FAILURE_FALLBACK_WINDOW: Duration = Duration::from_secs(30);

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
        quota_exhausted: false,
    },
    FailoverPattern {
        pattern: GEMINI_RETRY_CHAIN_EXHAUSTION_PATTERNS[1],
        reason: "gemini_retry_chain_exhausted",
        advance_to_next_model: true,
        quota_exhausted: false,
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

    let stderr_lower = stderr.to_ascii_lowercase();
    let stdout_lower = stdout.to_ascii_lowercase();
    let combined_lower = format!("{stderr_lower}\n{stdout_lower}");
    for pattern in patterns_for_tool(tool_name)
        .iter()
        .chain(failover_patterns_for_tool(tool_name).iter())
    {
        // Always search combined (stderr+stdout) so codex quota errors emitted
        // as JSON on stdout are detected. The exit_code!=0 precondition already
        // filters normal successful output.
        //
        // quota_exhausted is only confirmed when the pattern also appears in
        // stderr (stdout-only matches could be tool output echoing the phrase,
        // so we still failover but don't mark it as permanent quota exhaustion).
        if combined_lower.contains(pattern.pattern) {
            let confirmed_quota = pattern.quota_exhausted && stderr_lower.contains(pattern.pattern);
            return Some(RateLimitDetected {
                tool: tool_name.to_string(),
                matched_pattern: public_failover_marker(pattern, confirmed_quota),
                reason: pattern.reason.to_string(),
                advance_to_next_model: pattern.advance_to_next_model,
                quota_exhausted: confirmed_quota,
                model_spec: model_spec.map(String::from),
            });
        }
    }

    if let Some(http_status) = detect_http_status_failover(&combined_lower) {
        return Some(RateLimitDetected {
            tool: tool_name.to_string(),
            matched_pattern: http_status.matched_pattern.clone(),
            reason: http_status.reason,
            advance_to_next_model: true,
            quota_exhausted: false,
            model_spec: model_spec.map(String::from),
        });
    }

    None
}

pub fn requires_init_failure_window(detected: &RateLimitDetected) -> bool {
    http_status_reason_requires_init_window(&detected.reason)
}

pub fn within_init_failure_window(elapsed: Duration) -> bool {
    elapsed <= INIT_FAILURE_FALLBACK_WINDOW
}

struct HttpStatusFailover {
    matched_pattern: String,
    reason: String,
}

fn detect_http_status_failover(text: &str) -> Option<HttpStatusFailover> {
    let mut previous_token: Option<&str> = None;
    for token in text.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        if let Some(marker @ ("http" | "status" | "statuscode")) = previous_token
            && let Some(reason) = http_failover_reason(token)
        {
            return Some(HttpStatusFailover {
                matched_pattern: format!("{marker} {token}"),
                reason,
            });
        }
        previous_token = Some(token);
    }
    None
}

fn http_failover_reason(token: &str) -> Option<String> {
    if matches!(token, "4xx" | "5xx") {
        return Some(format!("HTTP {token}"));
    }

    let code = token.parse::<u16>().ok()?;
    (400..=599).contains(&code).then(|| format!("HTTP {code}"))
}

fn http_status_reason_requires_init_window(reason: &str) -> bool {
    let Some(status) = reason.strip_prefix("HTTP ") else {
        return false;
    };
    if status == "4xx" || status == "5xx" {
        return true;
    }
    match status.parse::<u16>() {
        Ok(429 | 529) => false,
        Ok(code) => (400..=599).contains(&code),
        Err(_) => false,
    }
}

fn failover_patterns_for_tool(tool: &str) -> &'static [FailoverPattern] {
    match tool {
        // antigravity-cli rides the same Gemini retry chain as gemini-cli — its
        // ACP transport (when applicable) emits identical exhaustion markers.
        "gemini-cli" | "antigravity-cli" => GEMINI_RETRY_CHAIN_FAILOVER_PATTERNS,
        "codex" | "claude-code" => ACP_CRASH_FAILOVER_PATTERNS,
        _ => &[],
    }
}

fn patterns_for_tool(tool: &str) -> &'static [FailoverPattern] {
    match tool {
        // antigravity-cli shares Google's OAuth/API quota pool with gemini-cli,
        // so it surfaces the same RESOURCE_EXHAUSTED / quota messages. Reuse
        // the gemini-cli patterns verbatim so failover detection works for it
        // without a parallel pattern table (#1629).
        "gemini-cli" | "antigravity-cli" => &[
            FailoverPattern {
                pattern: "oauth browser prompt detected",
                reason: "auth_unavailable",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "opening authentication page in your browser",
                reason: "auth_unavailable",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "429_quota_exhausted",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "quota_exhausted",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "quota exhausted",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "monthly spending cap",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "monthly cap",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "spending cap",
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
                pattern: "quota exceeded",
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
                pattern: "resource exhausted",
                reason: "RESOURCE_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "resource_exhausted",
                reason: "RESOURCE_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "capacity exhausted",
                reason: "RESOURCE_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "capacity_exhausted",
                reason: "RESOURCE_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "exhausted your capacity",
                reason: "RESOURCE_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "no capacity available",
                reason: "RESOURCE_EXHAUSTED",
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
                pattern: "api key not found",
                reason: "auth_unavailable",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "_apierror: {\"error\":\"invalid api key\"}",
                reason: "auth_unavailable",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "invalid api key",
                reason: "auth_unavailable",
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
                pattern: "codex_429_retry_exhausted",
                reason: "codex_429_retry_exhausted",
                advance_to_next_model: true,
                quota_exhausted: true,
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
                pattern: "monthly spending cap",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "monthly cap",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "spending cap",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "billing limit",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "billing cap",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "billing disabled",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "billing not enabled",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "billing hard limit",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "billing budget",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "insufficient_quota",
                reason: "QUOTA_EXHAUSTED",
                advance_to_next_model: true,
                quota_exhausted: true,
            },
            FailoverPattern {
                pattern: "hard limit",
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
                pattern: "429_quota_exhausted",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
            },
            FailoverPattern {
                pattern: "rate_limit_exceeded",
                reason: "HTTP 429",
                advance_to_next_model: true,
                quota_exhausted: false,
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
                reason: "auth_unavailable",
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

fn public_failover_marker(pattern: &FailoverPattern, confirmed_quota: bool) -> String {
    if pattern.reason == "auth_unavailable" {
        return "auth_unavailable".to_string();
    }
    if !confirmed_quota && is_transient_quota_pattern(pattern.pattern) {
        return "rate-limit-429".to_string();
    }
    pattern.pattern.to_string()
}

fn is_transient_quota_pattern(pattern: &str) -> bool {
    matches!(
        pattern,
        "429_quota_exhausted" | "quota_exhausted" | "quota exhausted" | "quota exceeded"
    )
}

#[cfg(test)]
#[path = "rate_limit_tests.rs"]
mod tests;
