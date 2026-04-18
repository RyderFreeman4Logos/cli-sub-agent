/// ACP crash retry logic for non-gemini tools (Issue #567).
///
/// Claude-code and other ACP servers can crash with "server shut down
/// unexpectedly" during large diff reads. This module provides configurable
/// crash retry attempts, distinct from the gemini-specific rate-limit retry
/// chain in `transport_gemini_retry.rs`.
use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Result, anyhow};
use csa_session::state::{MetaSessionState, ToolState};

use super::{AcpTransport, TransportOptions, TransportResult};

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
    let lowered = error_display.to_lowercase();
    is_retryable_crash_lowered(&lowered)
}

/// Check if the error indicates an OOM or resource-limit kill.
///
/// Exposed as `pub(crate)` so the caller can provide enhanced OOM-specific
/// error messages even when no retry was attempted.
pub(crate) fn is_oom_error(error_display: &str) -> bool {
    let lowered = error_display.to_lowercase();
    is_oom_lowered(&lowered)
}

/// Check if the error indicates a deterministic auth or permission failure.
pub(crate) fn is_auth_error(error_display: &str) -> bool {
    let lowered = error_display.to_lowercase();
    is_auth_lowered(&lowered)
}

// ---------------------------------------------------------------------------
// Internal classifiers operating on pre-lowered strings.
// ---------------------------------------------------------------------------

/// Core retryable-crash classification on a pre-lowered string.
fn is_retryable_crash_lowered(lowered: &str) -> bool {
    // Never retry OOM / resource limit kills — the same limit will be hit again.
    if is_oom_lowered(lowered) {
        return false;
    }

    // Never retry auth/permission failures — the same credentials will fail again.
    if is_auth_lowered(lowered) {
        return false;
    }

    // Never retry configuration or spawn errors — these are deterministic.
    if is_config_or_spawn_lowered(lowered) {
        return false;
    }

    // Never retry timeout errors — the agent already consumed its time budget.
    if is_timeout_lowered(lowered) {
        return false;
    }

    // Retry server crashes and internal errors.
    is_crash_lowered(lowered)
}

/// OOM classification on a pre-lowered string.
fn is_oom_lowered(lowered: &str) -> bool {
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

/// Authentication or authorization failures (deterministic, never retry).
fn is_auth_lowered(lowered: &str) -> bool {
    lowered.contains("401 unauthorized")
        || lowered.contains("403 forbidden")
        || lowered.contains("insufficient permissions")
        || lowered.contains("missing scopes")
        || lowered.contains("missing_scope")
        || lowered.contains("unauthorized")
}

/// Configuration or spawn failure (deterministic, never retry).
fn is_config_or_spawn_lowered(lowered: &str) -> bool {
    lowered.contains("configuration error")
        || lowered.contains("spawn failed")
        || lowered.contains("binary not found")
        || lowered.contains("no such file or directory")
}

/// Timeout (already consumed time budget).
fn is_timeout_lowered(lowered: &str) -> bool {
    lowered.contains("timed out") || lowered.contains("idle timeout")
}

/// Server crash that might succeed on retry.
fn is_crash_lowered(lowered: &str) -> bool {
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

fn format_memory_limit(memory_max_mb: Option<u64>) -> String {
    match memory_max_mb {
        Some(limit_mb) => format!("{limit_mb}MB"),
        None => "(no explicit limit set — using system default)".to_string(),
    }
}

fn memory_limit_config_key(tool_name: &str) -> String {
    format!("[tools.{tool_name}].memory_max_mb")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexAcpCrashKind {
    Oom,
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrashExitCode {
    ExitCode(i32),
    Signal(i32),
    OomEvent,
}

impl CodexAcpCrashKind {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Oom => "codex_acp_crash_oom",
            Self::Runtime => "codex_acp_crash_runtime",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexAcpCrashClassification {
    pub(crate) kind: CodexAcpCrashKind,
    pub(crate) rendered_hint: String,
}

pub(crate) fn extract_crash_exit_code(error_str: &str) -> Option<CrashExitCode> {
    let lowered = error_str.to_ascii_lowercase();

    if lowered.contains("exit code 137") || lowered.contains("exit 137") {
        return Some(CrashExitCode::ExitCode(137));
    }

    if lowered.contains("signal: 9") || lowered.contains("sigkill") {
        return Some(CrashExitCode::Signal(9));
    }

    if lowered.contains("memory.max")
        || lowered.contains("oom-kill")
        || lowered.contains("memory.events")
        || lowered.contains("memory.events.local")
        || lowered.contains("oom ")
        || lowered.contains(" oom")
        || lowered.ends_with("oom")
    {
        return Some(CrashExitCode::OomEvent);
    }

    let exit_code_marker = "exit code ";
    if let Some(start) = lowered.find(exit_code_marker) {
        let digits: String = lowered[start + exit_code_marker.len()..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            return digits.parse::<i32>().ok().map(CrashExitCode::ExitCode);
        }
    }

    None
}

pub(crate) fn classify_codex_acp_crash(
    stderr_or_context: &str,
    exit_code: Option<CrashExitCode>,
    cgroup_state: Option<&str>,
    memory_max_mb: Option<u64>,
) -> CodexAcpCrashClassification {
    let stderr_lower = stderr_or_context.to_ascii_lowercase();
    let cgroup_lower = cgroup_state.map(str::to_ascii_lowercase);
    let config_key = memory_limit_config_key("codex");
    let current_limit = format_memory_limit(memory_max_mb);

    let exit_code_oom = matches!(
        exit_code,
        Some(CrashExitCode::ExitCode(137))
            | Some(CrashExitCode::Signal(9))
            | Some(CrashExitCode::OomEvent)
    );
    let stderr_exact_killed = stderr_lower
        .rsplit_once("stderr:")
        .is_some_and(|(_, tail)| tail.trim() == "killed");
    let plain_killed_line = stderr_or_context
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case("killed"));
    let cgroup_oom = cgroup_lower.as_deref().is_some_and(|state| {
        state.contains("memory.max")
            || state.contains("oom")
            || state.contains("out of memory")
            || state.contains("killed")
    });
    let oom_signature = is_oom_lowered(&stderr_lower)
        || stderr_exact_killed
        || plain_killed_line
        || cgroup_oom
        || exit_code_oom;

    let (kind, rendered_hint) = if oom_signature {
        (
            CodexAcpCrashKind::Oom,
            format!(
                "{}: Codex ACP child died post-init and looks OOM-killed. \
                 Current limit: {current_limit} ({config_key}). \
                 For read-only forensic / rg-heavy work, consider claude-code native Agent tool as a lower-memory alternative. \
                 Suggestions: (1) raise {config_key}, (2) reduce tool output/context size, (3) switch tools for this workload.",
                CodexAcpCrashKind::Oom.code()
            ),
        )
    } else {
        (
            CodexAcpCrashKind::Runtime,
            format!(
                "{}: Codex ACP child died post-init without a confirmed OOM signature. \
                 Current limit: {current_limit} ({config_key}). \
                 If this keeps reproducing, inspect stderr/output tail and reduce context or switch tools.",
                CodexAcpCrashKind::Runtime.code()
            ),
        )
    };

    CodexAcpCrashClassification {
        kind,
        rendered_hint,
    }
}

pub(crate) fn format_codex_acp_crash(
    classification: &CodexAcpCrashClassification,
    error: anyhow::Error,
    attempts: u8,
) -> anyhow::Error {
    let retry_prefix = if attempts > 1 {
        format!("ACP crash retry exhausted ({attempts} attempts) for codex. ")
    } else {
        String::new()
    };
    anyhow!(
        "{retry_prefix}{}\nOriginal error: {error:#}",
        classification.rendered_hint
    )
}

/// Format a user-facing error message for a non-retryable OOM crash.
pub(crate) fn format_oom_crash(
    error: anyhow::Error,
    tool_name: &str,
    memory_max_mb: Option<u64>,
) -> anyhow::Error {
    let current_limit = format_memory_limit(memory_max_mb);
    anyhow::anyhow!(
        "ACP process for {tool_name} was killed by signal 9 (OOM). This is not retryable.\n\
         Current limit: {current_limit} (configured at [tools.{tool_name}].memory_max_mb).\n\
         Suggestions:\n\
           1. Increase [tools.{tool_name}].memory_max_mb in .csa/config.toml or ~/.config/cli-sub-agent/config.toml\n\
           2. Reduce the task context/diff size\n\
           3. Switch to a lower-memory tool via --tier or --tool\n\
         \n\
         Original error: {error:#}"
    )
}

/// Format a user-facing error message for a non-retryable auth/config failure.
pub(crate) fn format_auth_failure(error: anyhow::Error, tool_name: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "ACP transport for {tool_name} failed due to authentication or permission error. \
         This is not retryable with the current credentials. \
         Consider: (1) verifying the tool's auth/session setup, \
         (2) checking required API scopes/roles, \
         (3) switching to a different tool via --tier if available. \
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
    let max_attempts = options.acp_crash_max_attempts.max(1);
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
                let memory_max_mb = options
                    .sandbox
                    .and_then(|sandbox| sandbox.isolation_plan.memory_max_mb);

                if is_retryable_acp_crash(&error_display) && attempt < max_attempts {
                    tracing::warn!(
                        attempt,
                        max_attempts,
                        tool = %transport.tool_name,
                        "ACP server crashed; retrying with fresh process \
                         in {ACP_CRASH_RETRY_DELAY_SECS}s"
                    );
                    tokio::time::sleep(Duration::from_secs(ACP_CRASH_RETRY_DELAY_SECS)).await;
                    attempt = attempt.saturating_add(1);
                    continue;
                }

                if is_auth_error(&error_display) {
                    return Err(format_auth_failure(error, &transport.tool_name));
                }
                if transport.tool_name == "codex" && is_crash_lowered(&error_display.to_lowercase())
                {
                    let classification = classify_codex_acp_crash(
                        &error_display,
                        extract_crash_exit_code(&error_display),
                        None,
                        memory_max_mb,
                    );
                    tracing::warn!(
                        classified_reason = classification.kind.code(),
                        memory_max_mb,
                        attempt,
                        max_attempts,
                        tool = %transport.tool_name,
                        "classified codex ACP crash"
                    );
                    return Err(format_codex_acp_crash(&classification, error, attempt));
                }
                if is_oom_error(&error_display) {
                    return Err(format_oom_crash(error, &transport.tool_name, memory_max_mb));
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

    #[test]
    fn test_unauthorized_missing_scope_is_not_retryable() {
        let err = "ACP prompt failed: Internal error: unexpected status 401 Unauthorized: \
                   You have insufficient permissions for this operation. \
                   Missing scopes: api.responses.write";
        assert!(is_auth_error(err));
        assert!(!is_retryable_acp_crash(err));
    }

    #[test]
    fn test_missing_scope_marker_is_not_retryable() {
        let err = "codex responses error: code=missing_scope";
        assert!(is_auth_error(err));
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
        let formatted = format_oom_crash(err, "codex", Some(4096));
        let msg = formatted.to_string();
        assert!(msg.contains("codex"));
        assert!(msg.contains("memory_max_mb"));
        assert!(msg.contains("4096MB"));
        assert!(msg.contains("Suggestions:"));
        assert!(msg.contains("1. Increase"));
        assert!(msg.contains("2. Reduce the task context/diff size"));
        assert!(msg.contains("3. Switch to a lower-memory tool via --tier or --tool"));
        assert!(msg.contains("--tier"));
        assert!(msg.contains("--tool"));
    }

    #[test]
    fn test_format_oom_crash_without_explicit_limit_mentions_system_default() {
        let err = anyhow::anyhow!("killed by signal 9 (SIGKILL)");
        let formatted = format_oom_crash(err, "codex", None);
        let msg = formatted.to_string();
        assert!(msg.contains("(no explicit limit set — using system default)"));
    }

    #[test]
    fn test_format_auth_failure_contains_key_info() {
        let err = anyhow::anyhow!("401 Unauthorized: Missing scopes: api.responses.write");
        let formatted = format_auth_failure(err, "codex");
        let msg = formatted.to_string();
        assert!(msg.contains("codex"));
        assert!(msg.contains("authentication or permission error"));
        assert!(msg.contains("scopes"));
        assert!(msg.contains("--tier"));
    }

    #[test]
    fn test_classify_codex_acp_crash_oom_from_killed_stderr() {
        let classification = classify_codex_acp_crash(
            "server shut down unexpectedly\nKilled\n",
            None,
            None,
            Some(6144),
        );
        assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
        assert!(classification.rendered_hint.contains("codex_acp_crash_oom"));
        assert!(classification.rendered_hint.contains("6144MB"));
        assert!(
            classification
                .rendered_hint
                .contains("[tools.codex].memory_max_mb")
        );
        assert!(
            classification
                .rendered_hint
                .contains("claude-code native Agent tool")
        );
    }
    #[test]
    fn test_classify_codex_acp_crash_oom_from_stderr_tail_killed() {
        let classification = classify_codex_acp_crash(
            "ACP prompt failed: Internal error: \"server shut down unexpectedly\"; stderr: Killed",
            None,
            None,
            Some(6144),
        );
        assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
    }
    #[test]
    fn test_classify_codex_acp_crash_oom_from_cgroup_state() {
        let classification = classify_codex_acp_crash(
            "server shut down unexpectedly",
            None,
            Some("memory.max exceeded"),
            Some(4096),
        );
        assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
        assert!(classification.rendered_hint.contains("4096MB"));
    }
    #[test]
    fn test_classify_codex_acp_crash_runtime_without_oom_signature() {
        let classification = classify_codex_acp_crash(
            "ACP prompt failed: Internal error: \"server shut down unexpectedly\"",
            None,
            None,
            Some(3072),
        );
        assert_eq!(classification.kind, CodexAcpCrashKind::Runtime);
        assert!(
            classification
                .rendered_hint
                .contains("codex_acp_crash_runtime")
        );
        assert!(classification.rendered_hint.contains("3072MB"));
        assert!(
            classification
                .rendered_hint
                .contains("[tools.codex].memory_max_mb")
        );
    }
    #[test]
    fn test_classify_codex_acp_crash_signal_11_stays_runtime() {
        let classification = classify_codex_acp_crash(
            "ACP process exited unexpectedly: killed by signal 11 (SIGSEGV)",
            None,
            None,
            Some(3072),
        );
        assert_eq!(classification.kind, CodexAcpCrashKind::Runtime);
    }
    #[test]
    fn test_classify_codex_acp_crash_signal_90_stays_runtime() {
        let classification = classify_codex_acp_crash(
            "ACP process exited unexpectedly: killed by signal 90",
            None,
            None,
            Some(3072),
        );
        assert_eq!(classification.kind, CodexAcpCrashKind::Runtime);
    }
    #[test]
    fn classify_extracts_exit_137_with_empty_stderr() {
        let classification = classify_codex_acp_crash(
            "ACP process exited unexpectedly: exit code 137",
            extract_crash_exit_code("ACP process exited unexpectedly: exit code 137"),
            None,
            Some(3072),
        );
        assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
    }
    #[test]
    fn classify_runtime_on_exit_1_with_oom_stderr() {
        let classification = classify_codex_acp_crash(
            "ACP process exited unexpectedly: exit code 1\nstderr: out of memory",
            extract_crash_exit_code("ACP process exited unexpectedly: exit code 1"),
            None,
            Some(3072),
        );
        assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
    }
    #[test]
    fn classify_runtime_on_exit_1_and_clean_stderr() {
        let classification = classify_codex_acp_crash(
            "ACP process exited unexpectedly: exit code 1",
            extract_crash_exit_code("ACP process exited unexpectedly: exit code 1"),
            None,
            Some(3072),
        );
        assert_eq!(classification.kind, CodexAcpCrashKind::Runtime);
    }
    #[test]
    fn classify_signal_9_as_oom() {
        let classification = classify_codex_acp_crash(
            "ACP process exited unexpectedly",
            extract_crash_exit_code("signal: 9"),
            None,
            Some(3072),
        );
        assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
    }
    #[test]
    fn extract_matches_oom_kill_literal() {
        assert_eq!(
            extract_crash_exit_code("cgroup memory.events reported oom-kill 1"),
            Some(CrashExitCode::OomEvent)
        );
    }
    #[test]
    fn extract_prefers_exit_137_over_generic_pattern() {
        assert_eq!(
            extract_crash_exit_code("process exited with exit code 137 after transport failure"),
            Some(CrashExitCode::ExitCode(137))
        );
    }
    #[test]
    fn test_format_codex_acp_crash_includes_hint_and_original_error() {
        let classification = classify_codex_acp_crash(
            "Killed",
            Some(CrashExitCode::ExitCode(137)),
            None,
            Some(2048),
        );
        let err = anyhow::anyhow!("server shut down unexpectedly");
        let formatted = format_codex_acp_crash(&classification, err, 1);
        let msg = formatted.to_string();
        assert!(msg.contains("codex_acp_crash_oom"));
        assert!(msg.contains("2048MB"));
        assert!(msg.contains("Original error"));
    }
    #[test]
    fn test_format_codex_acp_crash_retry_exhausted_preserves_failover_anchor() {
        let classification =
            classify_codex_acp_crash("server shut down unexpectedly", None, None, Some(2048));
        let err = anyhow::anyhow!("server shut down unexpectedly");
        let formatted = format_codex_acp_crash(&classification, err, 2);
        let msg = formatted.to_string().to_ascii_lowercase();
        assert!(msg.contains("acp crash retry exhausted"));
        assert!(msg.contains("codex_acp_crash_runtime"));
    }
}
