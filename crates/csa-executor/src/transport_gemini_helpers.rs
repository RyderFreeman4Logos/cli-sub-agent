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
