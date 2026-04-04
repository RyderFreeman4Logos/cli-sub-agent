/// ACP crash retry logic for non-gemini tools (Issue #567).
///
/// Claude-code and other ACP servers can crash with "server shut down
/// unexpectedly" during large diff reads. This module provides a single
/// retry attempt for crash-type errors, distinct from the gemini-specific
/// rate-limit retry chain in `transport_gemini_retry.rs`.
use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use csa_session::state::{MetaSessionState, ToolState};

use super::{AcpTransport, TransportOptions, TransportResult};

/// Maximum number of total attempts for ACP crash recovery.
/// One retry = two total attempts. We keep this minimal because crashes
/// during large context indicate a fundamental resource issue that
/// additional retries won't solve.
pub(crate) const ACP_CRASH_MAX_ATTEMPTS: u8 = 2;

/// Delay between crash retry attempts in seconds.
pub(crate) const ACP_CRASH_RETRY_DELAY_SECS: u64 = 3;

/// OOM-related signals that indicate the process was killed by the kernel
/// or resource sandbox. Retrying these wastes tokens because the same
/// resource limit will be hit again.
const OOM_SIGNALS: &[i32] = &[
    9, // SIGKILL — typically from OOM killer or cgroup enforcement
];

/// Classify whether an ACP error is a retryable crash.
///
/// Returns `true` for transient crashes (server shutdown, internal errors,
/// unexpected process exit with non-OOM signals). Returns `false` for
/// configuration errors, spawn failures, timeouts, and OOM kills.
pub(crate) fn is_retryable_acp_crash(error_display: &str) -> bool {
    // Never retry OOM / resource limit kills — the same limit will be hit again.
    if is_oom_error(error_display) {
        return false;
    }

    // Never retry configuration or spawn errors — these are deterministic.
    if is_config_or_spawn_error(error_display) {
        return false;
    }

    // Never retry timeout errors — the agent already consumed its time budget.
    if is_timeout_error(error_display) {
        return false;
    }

    // Retry server crashes and internal errors.
    is_crash_error(error_display)
}

/// Check if the error indicates an OOM or resource-limit kill.
///
/// Exposed as `pub(crate)` so the caller can provide enhanced OOM-specific
/// error messages even when no retry was attempted.
pub(crate) fn is_oom_error(error_display: &str) -> bool {
    let lowered = error_display.to_lowercase();

    // Signal-based OOM detection: "signal 9 (SIGKILL)" from ProcessExited display.
    // Word-boundary check: ensure the character after the number is NOT a digit,
    // preventing false matches like "signal 90" matching the pattern for signal 9.
    for &sig in OOM_SIGNALS {
        let pattern = format!("signal {sig}");
        if let Some(idx) = lowered.find(&pattern) {
            let next_char = lowered.as_bytes().get(idx + pattern.len());
            if next_char.is_none_or(|&c| !c.is_ascii_digit()) {
                return true;
            }
        }
    }

    // Explicit OOM hints from sandbox memory monitor
    lowered.contains("oom detected")
        || lowered.contains("out of memory")
        || lowered.contains("memory.max")
        || lowered.contains("memorymax")
}

/// Check if the error is a configuration or spawn failure (deterministic, never retry).
fn is_config_or_spawn_error(error_display: &str) -> bool {
    let lowered = error_display.to_lowercase();
    lowered.contains("configuration error")
        || lowered.contains("spawn failed")
        || lowered.contains("binary not found")
        || lowered.contains("no such file or directory")
}

/// Check if the error is a timeout (already consumed time budget).
fn is_timeout_error(error_display: &str) -> bool {
    let lowered = error_display.to_lowercase();
    lowered.contains("timed out") || lowered.contains("idle timeout")
}

/// Check if the error indicates a server crash that might succeed on retry.
fn is_crash_error(error_display: &str) -> bool {
    let lowered = error_display.to_lowercase();

    // Claude-code ACP server crash (the primary Issue #567 scenario)
    lowered.contains("shut down unexpectedly")
        || lowered.contains("server shut down")
        // Generic ACP internal errors
        || lowered.contains("internal error")
        // Process exited unexpectedly (non-OOM, already filtered above)
        || lowered.contains("process exited unexpectedly")
        // Connection-level failures (broken pipe, connection reset)
        || lowered.contains("broken pipe")
        || lowered.contains("connection reset")
        // Prompt-level transient failures
        || (lowered.contains("prompt failed") && lowered.contains("shut down"))
}

/// Format a user-facing error message for a crash that exhausted all retry attempts.
pub(crate) fn format_crash_retry_exhausted(
    error: anyhow::Error,
    tool_name: &str,
    attempts: u8,
) -> anyhow::Error {
    anyhow::anyhow!(
        "ACP crash retry exhausted ({attempts} attempts) for {tool_name}. \
         The ACP server crashed and did not recover after retry. \
         Consider: (1) reducing diff/context size, \
         (2) switching to a different tool via --tier, \
         (3) see https://github.com/RyderFreeman4Logos/cli-sub-agent/issues/567. \
         Last error: {error:#}"
    )
}

/// Format a user-facing error message for a non-retryable OOM crash.
pub(crate) fn format_oom_crash(error: anyhow::Error, tool_name: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "ACP process for {tool_name} was killed (likely OOM). \
         This is not retryable — the process hit a resource limit. \
         Consider: (1) increasing memory limits in .csa/config.toml [resources], \
         (2) reducing diff/context size, \
         (3) switching to a different tool via --tier. \
         Error: {error:#}"
    )
}

/// Execute an ACP prompt with crash retry for non-gemini tools.
///
/// ACP servers (claude-code, codex) can crash with "server shut down
/// unexpectedly" during large diff reads. One retry with a fresh process
/// often succeeds because the crash is transient.
pub(super) async fn execute_with_crash_retry(
    transport: &AcpTransport,
    prompt: &str,
    tool_state: Option<&ToolState>,
    session: &MetaSessionState,
    extra_env: Option<&HashMap<String, String>>,
    options: &TransportOptions<'_>,
) -> Result<TransportResult> {
    let mut attempt = 1u8;
    loop {
        // Only resume provider session on the first attempt; retries start fresh.
        let resume_id = if attempt == 1 {
            tool_state.and_then(|s| s.provider_session_id.clone())
        } else {
            None
        };
        if let Some(ref sid) = resume_id {
            tracing::debug!(session_id = %sid, "resuming ACP session from tool state");
        }

        let result = transport
            .execute_acp_attempt(
                prompt,
                session,
                extra_env,
                options,
                &transport.acp_args,
                resume_id.as_deref(),
            )
            .await;

        match result {
            Ok(tr) => return Ok(tr),
            Err(error) => {
                let error_display = format!("{error:#}");

                if is_retryable_acp_crash(&error_display) && attempt < ACP_CRASH_MAX_ATTEMPTS {
                    tracing::warn!(
                        attempt,
                        max_attempts = ACP_CRASH_MAX_ATTEMPTS,
                        tool = %transport.tool_name,
                        "ACP server crashed; retrying with fresh process \
                         in {ACP_CRASH_RETRY_DELAY_SECS}s"
                    );
                    tokio::time::sleep(Duration::from_secs(ACP_CRASH_RETRY_DELAY_SECS)).await;
                    attempt = attempt.saturating_add(1);
                    continue;
                }

                if is_oom_error(&error_display) {
                    return Err(format_oom_crash(error, &transport.tool_name));
                }
                if attempt > 1 {
                    return Err(format_crash_retry_exhausted(
                        error,
                        &transport.tool_name,
                        attempt,
                    ));
                }
                return Err(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Retryable crash scenarios ---

    #[test]
    fn test_server_shut_down_unexpectedly_is_retryable() {
        let err = "ACP prompt failed: server shut down unexpectedly";
        assert!(is_retryable_acp_crash(err));
    }

    #[test]
    fn test_internal_error_is_retryable() {
        let err = "ACP transport failed: ACP prompt failed: Internal error";
        assert!(is_retryable_acp_crash(err));
    }

    #[test]
    fn test_process_exited_non_oom_is_retryable() {
        // SIGSEGV (signal 11) — not in OOM_SIGNALS, so retryable
        let err = "ACP process exited unexpectedly: killed by signal 11 (SIGSEGV)";
        assert!(is_retryable_acp_crash(err));
    }

    #[test]
    fn test_broken_pipe_is_retryable() {
        let err = "ACP transport failed: Broken pipe";
        assert!(is_retryable_acp_crash(err));
    }

    #[test]
    fn test_connection_reset_is_retryable() {
        let err = "sandboxed ACP: ACP prompt failed: connection reset by peer";
        assert!(is_retryable_acp_crash(err));
    }

    #[test]
    fn test_prompt_failed_with_shut_down_is_retryable() {
        let err = "ACP prompt failed: the server shut down while processing";
        assert!(is_retryable_acp_crash(err));
    }

    // --- Non-retryable: OOM ---

    #[test]
    fn test_signal_9_oom_is_not_retryable() {
        let err = "ACP process exited unexpectedly: killed by signal 9 (SIGKILL)";
        assert!(!is_retryable_acp_crash(err));
    }

    #[test]
    fn test_oom_detected_is_not_retryable() {
        let err = "sandboxed ACP: ACP process exited unexpectedly: code 137; \
                   stderr:\nOOM detected: cgroup memory.max exceeded";
        assert!(!is_retryable_acp_crash(err));
    }

    #[test]
    fn test_out_of_memory_is_not_retryable() {
        let err = "ACP transport failed: out of memory";
        assert!(!is_retryable_acp_crash(err));
    }

    #[test]
    fn test_signal_90_is_not_oom_false_positive() {
        // "signal 90" should NOT match the OOM pattern for signal 9.
        let err = "ACP process exited unexpectedly: killed by signal 90";
        assert!(!is_oom_error(err));
        // It should be retryable since it's an unexpected exit, not OOM.
        assert!(is_retryable_acp_crash(err));
    }

    #[test]
    fn test_memory_max_exceeded_is_oom() {
        let err = "cgroup memory.max exceeded, process killed";
        assert!(is_oom_error(err));
        assert!(!is_retryable_acp_crash(err));
    }

    // --- Non-retryable: Config/Spawn ---

    #[test]
    fn test_config_error_is_not_retryable() {
        let err = "Configuration error: missing API key";
        assert!(!is_retryable_acp_crash(err));
    }

    #[test]
    fn test_spawn_failed_is_not_retryable() {
        let err = "ACP subprocess spawn failed: No such file or directory";
        assert!(!is_retryable_acp_crash(err));
    }

    #[test]
    fn test_binary_not_found_is_not_retryable() {
        let err = "ACP subprocess spawn failed: binary not found";
        assert!(!is_retryable_acp_crash(err));
    }

    // --- Non-retryable: Timeout ---

    #[test]
    fn test_timed_out_is_not_retryable() {
        let err = "ACP prompt failed: timed out waiting for response";
        assert!(!is_retryable_acp_crash(err));
    }

    #[test]
    fn test_idle_timeout_is_not_retryable() {
        let err = "ACP prompt failed: idle timeout exceeded";
        assert!(!is_retryable_acp_crash(err));
    }

    // --- Non-retryable: unrelated errors ---

    #[test]
    fn test_generic_error_is_not_retryable() {
        let err = "some random error message";
        assert!(!is_retryable_acp_crash(err));
    }

    #[test]
    fn test_session_failed_is_not_retryable() {
        let err = "ACP session creation failed: invalid session ID";
        assert!(!is_retryable_acp_crash(err));
    }

    // --- Error formatting ---

    #[test]
    fn test_format_crash_retry_exhausted_contains_key_info() {
        let err = anyhow::anyhow!("server shut down unexpectedly");
        let formatted = format_crash_retry_exhausted(err, "claude-code", 2);
        let msg = formatted.to_string();
        assert!(msg.contains("2 attempts"));
        assert!(msg.contains("claude-code"));
        assert!(msg.contains("567"));
        assert!(msg.contains("--tier"));
    }

    #[test]
    fn test_format_oom_crash_contains_key_info() {
        let err = anyhow::anyhow!("killed by signal 9 (SIGKILL)");
        let formatted = format_oom_crash(err, "codex");
        let msg = formatted.to_string();
        assert!(msg.contains("codex"));
        assert!(msg.contains("memory limits"));
        assert!(msg.contains("--tier"));
    }
}
