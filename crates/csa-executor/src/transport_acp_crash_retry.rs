/// ACP crash retry logic for non-gemini tools (Issue #567).
///
/// Claude-code and other ACP servers can crash with "server shut down
/// unexpectedly" during large diff reads. This module provides configurable
/// crash retry attempts, distinct from the gemini-specific rate-limit retry
/// chain in `transport_gemini_retry.rs`.
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Result, anyhow};
use csa_core::transport_events::SessionEvent;
use csa_session::state::{MetaSessionState, ToolState};
use regex::Regex;

use crate::model_spec::ThinkingBudget;
use csa_core::env::NO_FAILOVER_ENV_KEY;

use super::{
    AcpTransport, DEFAULT_CODEX_INITIAL_RESPONSE_TIMEOUT_SECONDS, TransportOptions,
    TransportResult, consume_resolved_initial_response_timeout_seconds,
    transport_gemini_helpers::strip_acp_timeout_footer,
};

/// Delay between crash retry attempts in seconds.
pub(crate) const ACP_CRASH_RETRY_DELAY_SECS: u64 = 3;
pub(crate) const CODEX_ACP_INITIAL_STALL_REASON: &str = "codex_acp_initial_stall";

/// Detect whether a successful `TransportResult` represents an ACP idle disconnect.
///
/// Idle disconnect is characterized by exit_code=137 and "idle timeout" in stderr,
/// which the csa-acp layer produces when the ACP process is killed due to no
/// events/stderr within the configured idle timeout window. This is distinct from
/// OOM kills (which also exit 137 but have different stderr signatures) and from
/// initial response timeouts.
pub(crate) fn is_idle_disconnect(result: &TransportResult) -> bool {
    result.execution.exit_code == 137
        && result
            .execution
            .stderr_output
            .contains("idle timeout: no ACP events/stderr for")
}

/// Build ACP args with reasoning effort injected or replaced for the downshifted budget.
///
/// If the args already contain `model_reasoning_effort=...`, replace it.
/// Otherwise, inject `-c model_reasoning_effort=<effort>` (codex-style).
fn build_downshifted_acp_args(args: &[String], new_budget: &ThinkingBudget) -> Vec<String> {
    let new_effort = new_budget.codex_effort();
    let effort_prefix = "model_reasoning_effort=";
    let mut result = Vec::with_capacity(args.len() + 2);
    let mut replaced = false;
    for arg in args {
        if arg.starts_with(effort_prefix) {
            result.push(format!("{effort_prefix}{new_effort}"));
            replaced = true;
        } else {
            result.push(arg.clone());
        }
    }
    if !replaced {
        // Inject codex-style `-c model_reasoning_effort=<effort>` for adapters that forward it.
        result.push("-c".to_string());
        result.push(format!("{effort_prefix}{new_effort}"));
    }
    result
}

/// OOM-related signals that indicate the process was killed by the kernel
/// or resource sandbox. Retrying these wastes tokens because the same
/// resource limit will be hit again.
const OOM_SIGNALS: &[i32] = &[
    9, // SIGKILL — typically from OOM killer or cgroup enforcement
];

static EXIT_137_RE: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bexit(?:\s+code)?[\s:=]+137\b"));
static EXIT_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bexit(?:\s+code)?[\s:=]+(?P<code>-?\d+)\b"));
static BARE_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bcode[\s:=]+(?P<code>-?\d+)\b"));
static OOM_WORD_RE: LazyLock<Regex> = LazyLock::new(|| compile_regex(r"\boom\b"));
static SIGNAL_9_RE: LazyLock<Regex> = LazyLock::new(|| compile_regex(r"\bsignal\s*[:\-]?\s*9\b"));

fn compile_regex(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(error) => panic!("invalid regex literal `{pattern}`: {error}"),
    }
}

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
        || OOM_WORD_RE.is_match(lowered)
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

    if EXIT_137_RE.is_match(&lowered) {
        return Some(CrashExitCode::ExitCode(137));
    }

    if SIGNAL_9_RE.is_match(&lowered) || lowered.contains("sigkill") {
        return Some(CrashExitCode::Signal(9));
    }

    if lowered.contains("oom-kill")
        || lowered.contains("memory.events")
        || lowered.contains("memory.events.local")
        || is_oom_lowered(&lowered)
    {
        return Some(CrashExitCode::OomEvent);
    }

    if let Some(captures) = EXIT_CODE_RE.captures(&lowered)
        && let Some(code) = captures.name("code")
    {
        return code
            .as_str()
            .parse::<i32>()
            .ok()
            .map(CrashExitCode::ExitCode);
    }

    if let Some(captures) = BARE_CODE_RE.captures(&lowered)
        && let Some(code) = captures.name("code")
    {
        return code
            .as_str()
            .parse::<i32>()
            .ok()
            .map(CrashExitCode::ExitCode);
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
            | Some(CrashExitCode::ExitCode(-9))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodexAcpInitialStallClassification {
    pub(crate) timeout_seconds: u64,
}

fn event_counts_as_codex_acp_initial_response(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::AgentMessage(_)
            | SessionEvent::AgentThought(_)
            | SessionEvent::PlanUpdate(_)
            | SessionEvent::ToolCallStarted { .. }
            | SessionEvent::ToolCallCompleted { .. }
    )
}

pub(crate) fn classify_codex_acp_initial_stall(
    result: &TransportResult,
    timeout_seconds: Option<u64>,
) -> Option<CodexAcpInitialStallClassification> {
    if result.execution.exit_code != 137
        || !result.execution.output.is_empty()
        || !result
            .execution
            .summary
            .starts_with("initial response timeout:")
        || result
            .events
            .iter()
            .any(event_counts_as_codex_acp_initial_response)
        || !strip_acp_timeout_footer(&result.execution.stderr_output)
            .trim()
            .is_empty()
    {
        return None;
    }

    Some(CodexAcpInitialStallClassification {
        timeout_seconds: timeout_seconds.unwrap_or(DEFAULT_CODEX_INITIAL_RESPONSE_TIMEOUT_SECONDS),
    })
}

pub(crate) fn apply_codex_acp_initial_stall_summary(
    execution: &mut csa_process::ExecutionResult,
    classification: &CodexAcpInitialStallClassification,
    retry_attempted: bool,
) {
    let summary = format!(
        "{CODEX_ACP_INITIAL_STALL_REASON}: no AgentMessageChunk/AgentThought/PlanUpdate/ToolCall event within {}s (retry_attempted={retry_attempted})",
        classification.timeout_seconds
    );
    execution.summary = summary.clone();
    if !execution.stderr_output.is_empty() && !execution.stderr_output.ends_with('\n') {
        execution.stderr_output.push('\n');
    }
    execution.stderr_output.push_str(&summary);
    execution.stderr_output.push('\n');
}

/// Execute an ACP prompt with crash retry for non-gemini tools.
///
/// ACP servers (claude-code, codex) can crash with "server shut down
/// unexpectedly" during large diff reads. One retry with a fresh process
/// often succeeds because the crash is transient.
///
/// When `options.thinking_budget` is set, idle disconnects trigger an automatic
/// one-level downshift and a single retry with reduced reasoning effort
/// (Issue #766). `--no-failover` (via `_CSA_NO_FAILOVER` in `extra_env`)
/// disables this behavior.
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
    let codex_initial_response_timeout_seconds = (transport.tool_name == "codex")
        .then(|| {
            consume_resolved_initial_response_timeout_seconds(options.initial_response_timeout)
        })
        .flatten();
    let no_failover = extra_env.is_some_and(|env| env.contains_key(NO_FAILOVER_ENV_KEY));
    let mut idle_downshift_attempted = false;
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
            Ok(mut tr) => {
                // --- Issue #766: idle disconnect auto-downshift ---
                // On first idle disconnect, if a downshift target exists and
                // --no-failover is not set, retry once with reduced reasoning effort.
                if is_idle_disconnect(&tr) && !idle_downshift_attempted && !no_failover {
                    idle_downshift_attempted = true;
                    if let Some(downshift_target) = options
                        .thinking_budget
                        .as_ref()
                        .and_then(ThinkingBudget::idle_disconnect_downshift)
                    {
                        let downshifted_args =
                            build_downshifted_acp_args(&transport.acp_args, &downshift_target);
                        tracing::warn!(
                            tool = %transport.tool_name,
                            from = ?options.thinking_budget,
                            to = ?downshift_target,
                            new_effort = downshift_target.codex_effort(),
                            "ACP idle disconnect detected; retrying with \
                             downshifted thinking budget (#766)"
                        );
                        tokio::time::sleep(Duration::from_secs(ACP_CRASH_RETRY_DELAY_SECS)).await;
                        // Single retry with downshifted args — route through
                        // classification instead of returning early (#1101).
                        match transport
                            .execute_acp_attempt(
                                prompt,
                                session,
                                extra_env,
                                options,
                                &downshifted_args,
                                None, // fresh process, no resume
                            )
                            .await
                        {
                            Ok(tr) => return Ok(tr),
                            Err(error) => {
                                let error_display = format!("{error:#}");
                                let memory_max_mb = options
                                    .sandbox
                                    .and_then(|sandbox| sandbox.isolation_plan.memory_max_mb);
                                if is_auth_error(&error_display) {
                                    return Err(format_auth_failure(error, &transport.tool_name));
                                }
                                if transport.tool_name == "codex"
                                    && is_crash_lowered(&error_display.to_lowercase())
                                {
                                    let classification = classify_codex_acp_crash(
                                        &error_display,
                                        extract_crash_exit_code(&error_display),
                                        None,
                                        memory_max_mb,
                                    );
                                    return Err(format_codex_acp_crash(
                                        &classification,
                                        error,
                                        attempt + 1,
                                    ));
                                }
                                if is_oom_error(&error_display) {
                                    return Err(format_oom_crash(
                                        error,
                                        &transport.tool_name,
                                        memory_max_mb,
                                    ));
                                }
                                return Err(error);
                            }
                        }
                    }
                    // No downshift target (already at lowest) — fall through to return as-is.
                    tracing::warn!(
                        tool = %transport.tool_name,
                        budget = ?options.thinking_budget,
                        "ACP idle disconnect detected but thinking budget is \
                         already at minimum; propagating result (#766)"
                    );
                }

                if transport.tool_name == "codex"
                    && let Some(classification) = classify_codex_acp_initial_stall(
                        &tr,
                        codex_initial_response_timeout_seconds,
                    )
                {
                    tracing::warn!(
                        classified_reason = CODEX_ACP_INITIAL_STALL_REASON,
                        elapsed_seconds = classification.timeout_seconds,
                        attempt,
                        max_attempts,
                        "codex ACP initial-response stall detected"
                    );
                    if attempt < max_attempts {
                        tracing::info!(
                            attempt,
                            max_attempts,
                            "retrying codex ACP after initial-response stall"
                        );
                        tokio::time::sleep(Duration::from_secs(ACP_CRASH_RETRY_DELAY_SECS)).await;
                        attempt = attempt.saturating_add(1);
                        continue;
                    }

                    apply_codex_acp_initial_stall_summary(
                        &mut tr.execution,
                        &classification,
                        attempt > 1,
                    );
                }
                return Ok(tr);
            }
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
    include!("transport_tests_acp_crash_retry.rs");
}
