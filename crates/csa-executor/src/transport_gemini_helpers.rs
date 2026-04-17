use std::collections::HashMap;
use std::path::Path;

use anyhow::anyhow;

use csa_process::ExecutionResult;
use csa_resource::isolation_plan::IsolationPlan;

use crate::transport_gemini_retry::{gemini_retry_model, gemini_should_use_api_key};

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
) {
    let Some(runtime_home) = runtime_home else {
        return;
    };

    let runtime_home_is_visible = isolation_plan.writable_paths.iter().any(|existing| {
        existing == runtime_home
            || (existing != Path::new("/tmp") && runtime_home.starts_with(existing))
    });
    if runtime_home_is_visible {
        return;
    }

    isolation_plan
        .writable_paths
        .push(runtime_home.to_path_buf());
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
