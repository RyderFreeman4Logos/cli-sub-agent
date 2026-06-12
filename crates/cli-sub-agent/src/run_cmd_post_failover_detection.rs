use std::time::Duration;

use tracing::warn;

#[derive(Clone, Copy)]
pub(super) enum TransportErrorFailoverKind {
    RateLimit,
    AcpCrashRetryExhausted,
    GeminiRetryChainExhausted,
    GeminiLegacyInitialStall,
}

pub(super) struct TransportErrorFailoverSignal {
    pub(super) kind: TransportErrorFailoverKind,
    pub(super) matched_pattern: String,
    pub(super) reason: String,
    pub(super) quota_exhausted: bool,
    pub(super) requires_init_failure_window: bool,
}

pub(crate) fn format_tool_exhausted_summary(tool_name: &str, matched_pattern: &str) -> String {
    format!(
        "tool_exhausted: {tool_name} permanent quota exhaustion detected \
         (matched '{matched_pattern}'); no retry or tool fallback attempted. \
         Inspect the tool account billing/quota or choose another tool explicitly."
    )
}

pub(crate) fn detect_permanent_tool_exhaustion_result(
    tool_name_str: &str,
    exec_result: &csa_process::ExecutionResult,
    current_model_spec: Option<&str>,
) -> Option<csa_scheduler::RateLimitDetected> {
    // Only stderr_output is the provider's error channel; `summary`/`output`
    // are agent stdout (reviewed/echoed content) and must not drive a permanent
    // quota verdict (#1736).
    detect_permanent_tool_exhaustion_text(
        tool_name_str,
        &exec_result.stderr_output,
        exec_result.exit_code,
        current_model_spec,
    )
}

pub(crate) fn detect_permanent_tool_exhaustion_text(
    tool_name_str: &str,
    provider_error_channel: &str,
    exit_code: i32,
    current_model_spec: Option<&str>,
) -> Option<csa_scheduler::RateLimitDetected> {
    if exit_code == 0 {
        return None;
    }
    // Permanent self-kill must come only from the provider error channel, never
    // agent stdout/summary that may quote reviewed quota text (#1736).
    csa_scheduler::detect_rate_limit(
        tool_name_str,
        provider_error_channel,
        "",
        exit_code,
        current_model_spec,
    )
    .filter(|detected| detected.quota_exhausted)
    .filter(|detected| {
        is_provider_wide_quota_exhaustion(
            tool_name_str,
            detected.quota_exhausted,
            provider_error_channel,
        )
    })
}

pub(crate) fn is_permanent_tool_exhaustion_error(
    tool_name_str: &str,
    error_message: &str,
    current_model_spec: Option<&str>,
) -> bool {
    detect_transport_error_failover_signal(tool_name_str, error_message, current_model_spec)
        .is_some_and(|signal| signal.quota_exhausted)
}

pub(super) fn detect_transport_error_failover_signal(
    tool_name_str: &str,
    error_message: &str,
    current_model_spec: Option<&str>,
) -> Option<TransportErrorFailoverSignal> {
    let error_lower = error_message.to_ascii_lowercase();

    if error_lower.contains("acp crash retry exhausted")
        || error_lower.contains("crash retry exhausted")
    {
        let matched_pattern = if error_lower.contains("acp crash retry exhausted") {
            "acp crash retry exhausted"
        } else {
            "crash retry exhausted"
        };
        return Some(TransportErrorFailoverSignal {
            kind: TransportErrorFailoverKind::AcpCrashRetryExhausted,
            matched_pattern: matched_pattern.to_string(),
            reason: "acp_crash_retry_exhausted".to_string(),
            quota_exhausted: false,
            requires_init_failure_window: false,
        });
    }

    if error_lower.contains("gemini acp retry chain exhausted")
        || error_lower.contains("retry chain exhausted")
    {
        let matched_pattern = if error_lower.contains("gemini acp retry chain exhausted") {
            "gemini acp retry chain exhausted"
        } else {
            "retry chain exhausted"
        };
        return Some(TransportErrorFailoverSignal {
            kind: TransportErrorFailoverKind::GeminiRetryChainExhausted,
            matched_pattern: matched_pattern.to_string(),
            reason: "gemini_retry_chain_exhausted".to_string(),
            quota_exhausted: csa_core::gemini::detect_permanent_quota_exhaustion_pattern(
                error_message,
            )
            .is_some(),
            requires_init_failure_window: false,
        });
    }

    if tool_name_str == "gemini-cli" && error_lower.contains("gemini_legacy_initial_stall") {
        return Some(TransportErrorFailoverSignal {
            kind: TransportErrorFailoverKind::GeminiLegacyInitialStall,
            matched_pattern: "gemini_legacy_initial_stall".to_string(),
            reason: "gemini_legacy_initial_stall".to_string(),
            quota_exhausted: false,
            requires_init_failure_window: false,
        });
    }

    csa_scheduler::detect_rate_limit(
        tool_name_str,
        error_message,
        "",
        1, // synthetic non-zero exit code
        current_model_spec,
    )
    .map(|rate_limit| {
        let requires_init_failure_window = csa_scheduler::requires_init_failure_window(&rate_limit);
        TransportErrorFailoverSignal {
            kind: TransportErrorFailoverKind::RateLimit,
            matched_pattern: rate_limit.matched_pattern,
            reason: rate_limit.reason,
            quota_exhausted: rate_limit.quota_exhausted,
            requires_init_failure_window,
        }
    })
}

pub(super) fn allows_init_failure_failover(
    tool_name: &str,
    reason: &str,
    requires_init_failure_window: bool,
    attempt_elapsed: Option<Duration>,
) -> bool {
    if !requires_init_failure_window {
        return true;
    }
    let Some(elapsed) = attempt_elapsed else {
        return true;
    };
    if csa_scheduler::within_init_failure_window(elapsed) {
        warn!(
            tool = %tool_name,
            reason = %reason,
            elapsed_ms = elapsed.as_millis(),
            "[csa-failover] {tool_name} failed with {reason}, falling back to next tier model"
        );
        return true;
    }
    warn!(
        tool = %tool_name,
        reason = %reason,
        elapsed_ms = elapsed.as_millis(),
        "[csa-failover] HTTP failure occurred after initialization window; not attempting automatic tier fallback"
    );
    false
}

pub(super) fn is_provider_wide_quota_exhaustion(
    tool_name_str: &str,
    quota_exhausted: bool,
    provider_error_channel: &str,
) -> bool {
    quota_exhausted && !is_codex_model_scoped_usage_limit(tool_name_str, provider_error_channel)
}

fn is_codex_model_scoped_usage_limit(tool_name_str: &str, provider_error_channel: &str) -> bool {
    if tool_name_str != "codex" {
        return false;
    }

    let lower = provider_error_channel.to_ascii_lowercase();
    lower.contains("usage limit") && lower.contains("switch to another model")
}
