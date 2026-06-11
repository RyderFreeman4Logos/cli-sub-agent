//! Global caller hint configuration.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::global::GlobalConfig;
use crate::paths;

/// Default Codex shell `yield_time_ms` recommended for `csa session wait`.
pub const DEFAULT_CODEX_SESSION_WAIT_YIELD_MS: u64 = 300_000;
/// Default Codex MCP tool timeout to recommend for `csa_session_wait`.
pub const DEFAULT_CODEX_SESSION_WAIT_MCP_TOOL_TIMEOUT_SEC: u64 = 7_200;
/// Default internal MCP `csa_session_wait.timeout_seconds` cap.
///
/// This must stay below `DEFAULT_CODEX_SESSION_WAIT_MCP_TOOL_TIMEOUT_SEC` so
/// the server can return an alive/re-wait result before the caller's MCP tool
/// deadline cancels the request.
pub const DEFAULT_CODEX_SESSION_WAIT_MCP_INTERNAL_TIMEOUT_SEC: u64 = 6_900;

/// Configuration for hints emitted to parent tool callers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallerHintsConfig {
    /// Codex shell tool `yield_time_ms` to recommend for `csa session wait`.
    #[serde(default = "default_codex_session_wait_yield_ms")]
    pub codex_session_wait_yield_ms: u64,
}

const fn default_codex_session_wait_yield_ms() -> u64 {
    DEFAULT_CODEX_SESSION_WAIT_YIELD_MS
}

impl Default for CallerHintsConfig {
    fn default() -> Self {
        Self {
            codex_session_wait_yield_ms: default_codex_session_wait_yield_ms(),
        }
    }
}

impl CallerHintsConfig {
    pub fn is_default(&self) -> bool {
        self.codex_session_wait_yield_ms == default_codex_session_wait_yield_ms()
    }
}

impl GlobalConfig {
    /// Resolve the Codex caller hint `yield_time_ms` from global config.
    ///
    /// Missing, unreadable, or invalid config falls back to the conservative
    /// OpenAI prompt-cache retention default so hint emission never fails a CSA
    /// command.
    pub fn resolve_codex_session_wait_yield_ms() -> u64 {
        let config_dir = paths::config_dir();
        let path = config_dir.map(|dir| dir.join("config.toml"));
        Self::resolve_codex_session_wait_yield_ms_from_path(path.as_deref())
    }

    /// Resolve `[caller_hints].codex_session_wait_yield_ms` from an explicit path.
    ///
    /// This testable variant mirrors the runtime resolver while avoiding host
    /// config state in unit tests.
    pub fn resolve_codex_session_wait_yield_ms_from_path(path: Option<&Path>) -> u64 {
        let Some(path) = path else {
            return DEFAULT_CODEX_SESSION_WAIT_YIELD_MS;
        };

        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return DEFAULT_CODEX_SESSION_WAIT_YIELD_MS;
            }
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "Failed to read global config while resolving Codex caller hint yield interval"
                );
                return DEFAULT_CODEX_SESSION_WAIT_YIELD_MS;
            }
        };

        let raw: toml::Value = match toml::from_str(&content) {
            Ok(raw) => raw,
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "Failed to parse global config while resolving Codex caller hint yield interval"
                );
                return DEFAULT_CODEX_SESSION_WAIT_YIELD_MS;
            }
        };

        let Some(caller_hints) = raw.get("caller_hints").and_then(toml::Value::as_table) else {
            return DEFAULT_CODEX_SESSION_WAIT_YIELD_MS;
        };

        match caller_hints.get("codex_session_wait_yield_ms") {
            None => DEFAULT_CODEX_SESSION_WAIT_YIELD_MS,
            Some(value) => match value.as_integer() {
                Some(ms) if ms > 0 => match u64::try_from(ms) {
                    Ok(ms) => ms,
                    Err(_) => {
                        tracing::warn!(
                            path = %path.display(),
                            key = "caller_hints.codex_session_wait_yield_ms",
                            value = ms,
                            fallback = DEFAULT_CODEX_SESSION_WAIT_YIELD_MS,
                            "Ignoring out-of-range Codex caller hint yield interval; using default"
                        );
                        DEFAULT_CODEX_SESSION_WAIT_YIELD_MS
                    }
                },
                _ => {
                    tracing::warn!(
                        path = %path.display(),
                        key = "caller_hints.codex_session_wait_yield_ms",
                        fallback = DEFAULT_CODEX_SESSION_WAIT_YIELD_MS,
                        "Ignoring invalid Codex caller hint yield interval; using default"
                    );
                    DEFAULT_CODEX_SESSION_WAIT_YIELD_MS
                }
            },
        }
    }
}

#[cfg(test)]
#[path = "global_caller_hints_tests.rs"]
mod tests;
