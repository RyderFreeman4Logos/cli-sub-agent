use std::collections::HashMap;
use std::path::Path;

use anyhow::{Result, anyhow};

use csa_process::ExecutionResult;
use csa_resource::isolation_plan::IsolationPlan;

use super::transport_types::ResolvedTimeout;
use crate::executor::Executor;
use crate::transport_gemini_retry::{gemini_retry_model, gemini_should_use_api_key};

pub(crate) const GEMINI_OAUTH_PROMPT_SUMMARY: &str =
    "gemini-cli auth failure: OAuth browser prompt detected; no tool output produced";
pub(crate) const GEMINI_MCP_ISSUES_DETECTED_SUMMARY: &str =
    "MCP issues detected. Run /mcp list for status.";
pub(crate) const GEMINI_ACP_INITIAL_STALL_REASON: &str = "gemini_acp_initial_stall";
pub(crate) const GEMINI_LEGACY_INITIAL_STALL_REASON: &str = "gemini_legacy_initial_stall";
const DEFAULT_GEMINI_ACP_INITIAL_RESPONSE_TIMEOUT_SECONDS: u64 = 180;
const ACP_TIMEOUT_FOOTER_SUFFIX: &str = "s; process killed";

#[derive(Debug, Clone)]
pub(super) struct GeminiRetryPhase {
    pub(super) attempt: u8,
    auth_mode: &'static str,
    model: &'static str,
}

impl GeminiRetryPhase {
    pub(super) fn for_attempt(attempt: u8) -> Self {
        Self {
            attempt,
            auth_mode: if gemini_should_use_api_key(attempt) {
                "api_key"
            } else {
                "oauth"
            },
            model: gemini_retry_model(attempt).unwrap_or("inherit"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GeminiAcpInitialStallClassification {
    pub(super) code: &'static str,
    pub(super) timeout_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GeminiLegacyInitialStallClassification {
    pub(super) code: &'static str,
    pub(super) timeout_seconds: u64,
}

pub(super) fn gemini_acp_initial_response_timeout_seconds(
    tool_name: &str,
    resolved_timeout: ResolvedTimeout,
) -> Option<u64> {
    if tool_name != "gemini-cli" {
        return None;
    }

    resolved_timeout.as_option().filter(|seconds| *seconds > 0)
}

pub(super) fn classify_gemini_acp_initial_stall(
    execution: &ExecutionResult,
    timeout_seconds: Option<u64>,
) -> Option<GeminiAcpInitialStallClassification> {
    if !execution.output.is_empty()
        || execution.exit_code != 137
        || !execution.summary.starts_with("initial response timeout:")
    {
        return None;
    }
    if !strip_acp_timeout_footer(&execution.stderr_output)
        .trim()
        .is_empty()
    {
        return None;
    }

    Some(GeminiAcpInitialStallClassification {
        code: GEMINI_ACP_INITIAL_STALL_REASON,
        timeout_seconds: timeout_seconds
            .unwrap_or(DEFAULT_GEMINI_ACP_INITIAL_RESPONSE_TIMEOUT_SECONDS),
    })
}

pub(super) fn apply_gemini_acp_initial_stall_summary(
    execution: &mut ExecutionResult,
    classification: &GeminiAcpInitialStallClassification,
) {
    let summary = format!(
        "{reason}: no ACP events/stderr within {}s",
        classification.timeout_seconds,
        reason = classification.code
    );

    execution.summary = summary.clone();
    if !execution.stderr_output.is_empty() && !execution.stderr_output.ends_with('\n') {
        execution.stderr_output.push('\n');
    }
    execution.stderr_output.push_str(&summary);
    execution.stderr_output.push('\n');
}

pub(crate) fn strip_acp_timeout_footer(stderr: &str) -> &str {
    let trimmed = stderr.strip_suffix('\n').unwrap_or(stderr);
    if let Some((prefix, last_line)) = trimmed.rsplit_once('\n') {
        if is_acp_timeout_footer(last_line) {
            return prefix;
        }
    } else if is_acp_timeout_footer(trimmed) {
        return "";
    }

    stderr
}

fn is_acp_timeout_footer(line: &str) -> bool {
    [
        "initial response timeout: no ACP events/stderr for ",
        "idle timeout: no ACP events/stderr for ",
    ]
    .iter()
    .any(|prefix| {
        line.strip_prefix(prefix)
            .and_then(|suffix| suffix.strip_suffix(ACP_TIMEOUT_FOOTER_SUFFIX))
            .is_some_and(|seconds| {
                !seconds.is_empty() && seconds.bytes().all(|b| b.is_ascii_digit())
            })
    })
}

pub(super) fn classify_gemini_legacy_initial_stall(
    executor: &Executor,
    execution: &ExecutionResult,
    timeout_seconds: Option<u64>,
) -> Option<GeminiLegacyInitialStallClassification> {
    if !matches!(executor, Executor::GeminiCli { .. })
        || !execution.output.is_empty()
        || execution.exit_code != 137
        || !execution.summary.starts_with("initial_response_timeout:")
    {
        return None;
    }

    Some(GeminiLegacyInitialStallClassification {
        code: GEMINI_LEGACY_INITIAL_STALL_REASON,
        timeout_seconds: timeout_seconds.unwrap_or(120),
    })
}

pub(super) fn apply_gemini_legacy_initial_stall_summary(
    execution: &mut ExecutionResult,
    classification: &GeminiLegacyInitialStallClassification,
) {
    let summary = format!(
        "{reason}: no stdout within {}s",
        classification.timeout_seconds,
        reason = classification.code
    );

    execution.summary = summary.clone();
    if !execution.stderr_output.is_empty() && !execution.stderr_output.ends_with('\n') {
        execution.stderr_output.push('\n');
    }
    execution.stderr_output.push_str(&summary);
    execution.stderr_output.push('\n');
}

pub(crate) fn is_gemini_mcp_issue_result(execution: &ExecutionResult) -> bool {
    execution.exit_code != 0
        && [
            execution.summary.as_str(),
            execution.output.as_str(),
            execution.stderr_output.as_str(),
        ]
        .into_iter()
        .any(|text| {
            text.contains("MCP issues detected")
                || text.contains("Run /mcp list")
                || text.contains(GEMINI_MCP_ISSUES_DETECTED_SUMMARY)
        })
}

pub(crate) fn apply_gemini_mcp_warning_summary(
    execution: &mut ExecutionResult,
    warning_summary: &str,
) {
    execution.summary = if execution.summary.trim().is_empty() {
        warning_summary.to_string()
    } else {
        format!("{warning_summary} | {}", execution.summary.trim())
    };
    if !execution.stderr_output.is_empty() && !execution.stderr_output.ends_with('\n') {
        execution.stderr_output.push('\n');
    }
    execution.stderr_output.push_str(warning_summary);
    execution.stderr_output.push('\n');
}

pub(super) fn gemini_phase_desc(attempt: u8) -> &'static str {
    match attempt {
        1 => "OAuth->APIKey(same model)",
        2 => "APIKey(same model)->APIKey(flash)",
        3 => "APIKey(flash)",
        _ => "final",
    }
}

pub(super) fn append_gemini_retry_report(
    execution: &mut ExecutionResult,
    phases: &[GeminiRetryPhase],
) {
    if phases.len() <= 1 {
        return;
    }

    let report = format!("[gemini-retry] {}", format_gemini_retry_report(phases));
    if execution.stderr_output.is_empty() {
        execution.stderr_output = report;
        return;
    }

    if !execution.stderr_output.ends_with('\n') {
        execution.stderr_output.push('\n');
    }
    execution.stderr_output.push_str(&report);
}

pub(super) fn annotate_gemini_retry_error(
    error: anyhow::Error,
    phases: &[GeminiRetryPhase],
) -> anyhow::Error {
    if phases.len() <= 1 {
        return error;
    }

    anyhow!(
        "Gemini ACP retry chain exhausted. {}. Last error: {error:#}",
        format_gemini_retry_report(phases)
    )
}

pub(super) fn ensure_gemini_runtime_home_writable_path(
    isolation_plan: &mut IsolationPlan,
    runtime_home: Option<&Path>,
) -> bool {
    let Some(runtime_home) = runtime_home else {
        return false;
    };

    let runtime_home_is_visible = isolation_plan.writable_paths.iter().any(|existing| {
        existing == runtime_home
            || (existing != Path::new("/tmp") && runtime_home.starts_with(existing))
    });
    if runtime_home_is_visible {
        return true;
    }

    isolation_plan.add_writable_dir_or_creatable_parent(runtime_home)
}

pub(super) fn gemini_sandbox_runtime_env_overrides(
    env: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut overrides = HashMap::new();
    for key in [
        "HOME",
        "PATH",
        // NOTE: Do NOT include "TMPDIR" here — the isolation plan already
        // sets TMPDIR=/tmp for bwrap sessions. Overriding it with the host's
        // TMPDIR would leak a read-only path into the sandbox (#704 review).
        "GEMINI_CLI_HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "npm_config_cache",
        "MISE_CACHE_DIR",
        "MISE_STATE_DIR",
        "MISE_SHIM",
        "MISE_SHIMS_DIR",
    ] {
        if let Some(value) = env.get(key) {
            overrides.insert(key.to_string(), value.clone());
        }
    }

    if let Some(auth_mode) = env.get(csa_core::gemini::AUTH_MODE_ENV_KEY) {
        overrides.insert(
            csa_core::gemini::AUTH_MODE_ENV_KEY.to_string(),
            auth_mode.clone(),
        );
    }

    // Force the sandboxed Gemini child to see exactly the auth-routing env
    // chosen for this attempt, overriding any systemd user-manager leakage.
    overrides.insert(
        csa_core::gemini::API_KEY_ENV.to_string(),
        env.get(csa_core::gemini::API_KEY_ENV)
            .cloned()
            .unwrap_or_default(),
    );
    overrides.insert(
        csa_core::gemini::BASE_URL_ENV.to_string(),
        env.get(csa_core::gemini::BASE_URL_ENV)
            .cloned()
            .unwrap_or_default(),
    );

    overrides
}

pub(super) fn apply_gemini_sandbox_runtime_env_overrides(
    isolation_plan: &mut IsolationPlan,
    env_overrides: &HashMap<String, String>,
) {
    isolation_plan.env_overrides.extend(
        env_overrides
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
}

pub(super) fn apply_gemini_sandbox_runtime_contract(
    isolation_plan: &mut IsolationPlan,
    runtime_home: Option<&Path>,
    env: &HashMap<String, String>,
) -> Result<()> {
    ensure_gemini_runtime_home_writable_path(isolation_plan, runtime_home);
    if let Some(shared_npm_cache) = env.get("npm_config_cache").map(Path::new)
        && !ensure_gemini_runtime_home_writable_path(isolation_plan, Some(shared_npm_cache))
    {
        return Err(anyhow!(
            "gemini-cli sandbox plan failed: denied path {} for intent \
             'bwrap writable bind for shared npm cache (#1047 Phase 1 optimization)'. \
             Set XDG_CACHE_HOME to a writable location, or add this path \
             (or a writable parent) to [filesystem_sandbox].writable_paths or \
             [tools.gemini-cli].filesystem_sandbox.writable_paths.",
            shared_npm_cache.display(),
        ));
    }
    let env_overrides = gemini_sandbox_runtime_env_overrides(env);
    apply_gemini_sandbox_runtime_env_overrides(isolation_plan, &env_overrides);
    Ok(())
}

pub(crate) fn is_gemini_oauth_prompt_result(execution: &ExecutionResult) -> bool {
    let stdout_has_auth_text =
        execution.output.contains("authentication") || execution.output.contains("Authentication");
    let stderr_has_auth_text = execution.stderr_output.contains("authentication")
        || execution.stderr_output.contains("Authentication");
    if !stdout_has_auth_text && !stderr_has_auth_text {
        return false;
    }

    let normalized_stdout = if stdout_has_auth_text {
        normalize_gemini_prompt_text(&execution.output)
    } else {
        String::new()
    };
    let normalized_stderr = if stderr_has_auth_text {
        normalize_gemini_prompt_text(&execution.stderr_output)
    } else {
        String::new()
    };
    let combined = if normalized_stderr.is_empty() {
        normalized_stdout.clone()
    } else if normalized_stdout.is_empty() {
        normalized_stderr.clone()
    } else {
        format!("{normalized_stdout}\n{normalized_stderr}")
    };

    if !contains_gemini_oauth_prompt(&combined) {
        return false;
    }

    !combined.lines().any(|line| {
        line.contains("\"type\":\"turn.completed\"")
            || line.contains("\"type\": \"turn.completed\"")
            || line.trim() == "turn.completed"
    })
}

pub(crate) fn classify_gemini_oauth_prompt_result(execution: &mut ExecutionResult) {
    execution.exit_code = 1;
    execution.summary = GEMINI_OAUTH_PROMPT_SUMMARY.to_string();
    if execution.stderr_output.is_empty() {
        execution.stderr_output = GEMINI_OAUTH_PROMPT_SUMMARY.to_string();
    } else if !execution
        .stderr_output
        .contains(GEMINI_OAUTH_PROMPT_SUMMARY)
    {
        if !execution.stderr_output.ends_with('\n') {
            execution.stderr_output.push('\n');
        }
        execution
            .stderr_output
            .push_str(GEMINI_OAUTH_PROMPT_SUMMARY);
    }
}

pub fn contains_gemini_oauth_prompt(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("opening authentication page in your browser")
        || (lower.contains("opening authentication page")
            && lower.contains("do you want to continue"))
        || (lower.contains("authentication page in your browser")
            && lower.contains("do you want to continue"))
}

pub fn normalize_gemini_prompt_text(text: &str) -> String {
    let mut cleaned = String::new();
    let mut in_guard = false;
    for raw_line in strip_ansi_escape_sequences(text).lines() {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.starts_with("<csa-caller-sa-guard") {
            in_guard = true;
            continue;
        }
        if trimmed.starts_with("</csa-caller-sa-guard>") {
            in_guard = false;
            continue;
        }
        if trimmed.starts_with("<csa-caller-prompt-injection") {
            in_guard = true;
            continue;
        }
        if trimmed.starts_with("</csa-caller-prompt-injection>") {
            in_guard = false;
            continue;
        }
        if in_guard
            || trimmed.is_empty()
            || trimmed.starts_with("[csa-hook]")
            || trimmed.starts_with("WARNING: weave.lock")
            || trimmed.starts_with("csa run context:")
            || trimmed.starts_with("Running scope as unit:")
        {
            continue;
        }
        let stripped = trimmed.strip_prefix("[stdout] ").unwrap_or(trimmed);
        cleaned.push_str(stripped);
        cleaned.push('\n');
    }
    cleaned
}

pub fn strip_ansi_escape_sequences(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            stripped.push(ch);
            continue;
        }
        if !matches!(chars.peek(), Some('[')) {
            continue;
        }
        let _ = chars.next();
        for next in chars.by_ref() {
            if ('@'..='~').contains(&next) {
                break;
            }
        }
    }
    stripped
}

/// Convert a `tokio::task::JoinError` into a descriptive `anyhow::Error`.
///
/// Broken-pipe panics (from `eprintln!` after the tool process closes stderr)
/// are rewritten into a clean message that mentions the tool died, instead of
/// surfacing the raw tokio panic trace.
pub(super) fn classify_join_error(e: tokio::task::JoinError) -> anyhow::Error {
    if e.is_panic() {
        let msg = match e.into_panic().downcast::<String>() {
            Ok(s) => *s,
            Err(any) => match any.downcast::<&str>() {
                Ok(s) => s.to_string(),
                Err(_) => "unknown panic".to_string(),
            },
        };
        if msg.contains("Broken pipe") || msg.contains("os error 32") {
            return anyhow!(
                "ACP transport: tool process terminated unexpectedly (broken pipe on stderr)"
            );
        }
        anyhow!("ACP transport: task panicked: {msg}")
    } else {
        anyhow!("ACP transport: task cancelled: {e}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GeminiAcpInitFailureClassification {
    pub(super) code: &'static str,
    pub(super) missing_env_vars: Vec<&'static str>,
}

pub(super) fn is_gemini_acp_init_failure(error: &str) -> bool {
    let error_lower = error.to_ascii_lowercase();
    error_lower.contains("acp initialization failed")
        || error_lower.contains("sandboxed acp: acp initialization failed")
}

pub(super) fn classify_gemini_acp_init_failure(
    error: &str,
    execution_env: &HashMap<String, String>,
) -> GeminiAcpInitFailureClassification {
    let error_lower = error.to_ascii_lowercase();
    let missing_env_vars = missing_gemini_auth_env_vars(execution_env);

    let code = if is_gemini_init_oom(&error_lower) {
        "gemini_acp_init_oom"
    } else if is_gemini_init_mcp_extension(&error_lower) {
        "gemini_acp_init_mcp_extension"
    } else if !missing_env_vars.is_empty() && is_gemini_init_auth_env(&error_lower) {
        "gemini_acp_init_auth_env"
    } else {
        "gemini_acp_init_handshake_timeout"
    };

    GeminiAcpInitFailureClassification {
        code,
        missing_env_vars,
    }
}

pub(super) fn format_gemini_acp_init_failure(
    classification: &GeminiAcpInitFailureClassification,
    error: anyhow::Error,
    memory_max_mb: Option<u64>,
) -> anyhow::Error {
    let current_limit = match memory_max_mb {
        Some(limit_mb) => format!("{limit_mb}MB"),
        None => "(no explicit limit set — using system default)".to_string(),
    };

    let detail = match classification.code {
        "gemini_acp_init_oom" => format!(
            "Gemini ACP child died before handshake and looks memory-constrained. Current limit: {current_limit} ([tools.gemini-cli].memory_max_mb)."
        ),
        "gemini_acp_init_auth_env" => format!(
            "Gemini ACP child died before handshake and auth/routing env may have been stripped: {}.",
            classification.missing_env_vars.join(", ")
        ),
        "gemini_acp_init_mcp_extension" => {
            "Gemini ACP child died before handshake while starting a Gemini MCP extension."
                .to_string()
        }
        _ => {
            "Gemini ACP child died before handshake or initialization never completed.".to_string()
        }
    };

    anyhow!(
        "{code}: {detail}\nOriginal error: {error:#}",
        code = classification.code
    )
}

fn missing_gemini_auth_env_vars(execution_env: &HashMap<String, String>) -> Vec<&'static str> {
    [
        csa_core::gemini::API_KEY_ENV,
        csa_core::gemini::BASE_URL_ENV,
    ]
    .into_iter()
    .filter(|key| {
        std::env::var_os(key).is_some()
            && execution_env
                .get(*key)
                .is_none_or(|value| value.trim().is_empty())
    })
    .collect()
}

fn is_gemini_init_oom(error_lower: &str) -> bool {
    error_lower.contains("oom detected")
        || error_lower.contains("out of memory")
        || error_lower.contains("killed by signal 9")
        || error_lower.contains("memory limit")
}

fn is_gemini_init_auth_env(error_lower: &str) -> bool {
    [
        "auth",
        "oauth",
        "credential",
        "api key",
        "unauthorized",
        "forbidden",
        "permission denied",
        "login",
        "401",
        "403",
    ]
    .iter()
    .any(|pattern| error_lower.contains(pattern))
}

fn is_gemini_init_mcp_extension(error_lower: &str) -> bool {
    error_lower.contains("mcp-server")
        || error_lower.contains("gemini-cli-security")
        || error_lower.contains("/extensions/")
        || (error_lower.contains("extension")
            && ["enoent", "eacces", "spawn", "failed to start"]
                .iter()
                .any(|pattern| error_lower.contains(pattern)))
}

pub(super) fn format_gemini_retry_report(phases: &[GeminiRetryPhase]) -> String {
    phases
        .iter()
        .map(|phase| {
            format!(
                "attempt={} phase=\"{}\" auth={} model={}",
                phase.attempt,
                gemini_phase_desc(phase.attempt),
                phase.auth_mode,
                phase.model
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}
