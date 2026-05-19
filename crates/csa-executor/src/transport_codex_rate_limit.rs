use std::time::Duration;

use csa_process::ExecutionResult;

pub(crate) const CODEX_RATE_LIMIT_MAX_RETRIES: u8 = 3;
pub(crate) const CODEX_RATE_LIMIT_RETRY_EXHAUSTED_REASON: &str = "codex_429_retry_exhausted";
const CODEX_PERMANENT_QUOTA_PATTERNS: &[&str] = &[
    CODEX_RATE_LIMIT_RETRY_EXHAUSTED_REASON,
    "usage_limit_exceeded",
    "usage limit",
    "billing limit",
    "billing cap",
    "billing disabled",
    "billing not enabled",
    "billing hard limit",
    "billing budget",
    "monthly limit",
    "monthly cap",
    "monthly spending",
    "monthly quota",
    "spending cap",
    "hard limit",
    "insufficient_quota",
    "daily quota",
];

pub(crate) fn codex_rate_limit_backoff(retry_count: u8) -> Duration {
    let capped = retry_count.min(CODEX_RATE_LIMIT_MAX_RETRIES.saturating_sub(1));
    Duration::from_secs(30 * 2_u64.pow(u32::from(capped)))
}

fn is_codex_rate_limited_text(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    lowered.contains("429")
        || lowered.contains("too many requests")
        || lowered.contains("rate_limit_exceeded")
        || lowered.contains("ratelimiterror")
        || lowered.contains("retry-after")
        || lowered.contains("retry after")
}

fn is_codex_permanent_quota_text(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    CODEX_PERMANENT_QUOTA_PATTERNS
        .iter()
        .any(|pattern| lowered.contains(pattern))
}

pub(crate) fn is_codex_transient_rate_limit_text(text: &str) -> bool {
    is_codex_rate_limited_text(text) && !is_codex_permanent_quota_text(text)
}

pub(crate) fn is_codex_transient_rate_limit_result(execution: &ExecutionResult) -> bool {
    execution.exit_code != 0 && is_codex_transient_rate_limit_text(&execution.stderr_output)
}

pub(crate) fn is_codex_permanent_quota_result(execution: &ExecutionResult) -> bool {
    execution.exit_code != 0 && is_codex_permanent_quota_text(&execution.stderr_output)
}

pub(crate) fn apply_codex_rate_limit_retry_exhausted_summary(
    execution: &mut ExecutionResult,
    retries: u8,
) {
    let summary = format!(
        "{CODEX_RATE_LIMIT_RETRY_EXHAUSTED_REASON}: temporary codex 429 rate limit persisted after {retries} retries"
    );
    execution.summary = summary.clone();
    if !execution.stderr_output.is_empty() && !execution.stderr_output.ends_with('\n') {
        execution.stderr_output.push('\n');
    }
    execution.stderr_output.push_str(&summary);
    execution.stderr_output.push('\n');
}

#[cfg(test)]
#[path = "transport_codex_rate_limit_tests.rs"]
mod tests;
