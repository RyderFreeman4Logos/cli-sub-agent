//! ACP session meta construction, environment building, and sandboxed execution.
//!
//! Extracted from `transport.rs` to keep module sizes manageable.
//! This module handles meta/env construction and the sandboxed ACP run helper,
//! while `transport.rs` retains trait definitions and Transport dispatch.

#[cfg(feature = "acp")]
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(feature = "acp")]
use csa_core::env::{CSA_PARENT_SESSION_DIR_ENV_KEY, CSA_SESSION_DIR_ENV_KEY};
use csa_resource::isolation_plan::IsolationPlan;
#[cfg(feature = "acp")]
use csa_session::state::MetaSessionState;
#[cfg(feature = "acp")]
use serde_json::{Map, Value, json};

#[cfg(feature = "acp")]
use super::AcpTransport;
#[cfg(feature = "acp")]
use crate::hermes_config::HermesRunConfig;
#[cfg(feature = "acp")]
use crate::lefthook_guard::sanitize_env_map_for_codex;
#[cfg(feature = "acp")]
use crate::session_config::SessionConfig;

#[cfg(feature = "acp")]
const CSA_SESSION_ID_ENV: &str = "CSA_SESSION_ID";
#[cfg(feature = "acp")]
const CSA_DEPTH_ENV: &str = "CSA_DEPTH";
#[cfg(feature = "acp")]
const CSA_PROJECT_ROOT_ENV: &str = "CSA_PROJECT_ROOT";
#[cfg(feature = "acp")]
const CSA_INTERNAL_INVOCATION_ENV: &str = csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY;
#[cfg(feature = "acp")]
const CSA_TOOL_ENV: &str = "CSA_TOOL";
#[cfg(feature = "acp")]
const CSA_IS_SUBPROCESS_ENV: &str = "CSA_IS_SUBPROCESS";
#[cfg(feature = "acp")]
const CSA_PARENT_TOOL_ENV: &str = "CSA_PARENT_TOOL";
#[cfg(feature = "acp")]
const CSA_PARENT_SESSION_ENV: &str = "CSA_PARENT_SESSION";
#[cfg(feature = "acp")]
const CSA_DAEMON_SESSION_DIR_ENV: &str = "CSA_DAEMON_SESSION_DIR";
#[cfg(feature = "acp")]
const CSA_FS_SANDBOXED_ENV: &str = "CSA_FS_SANDBOXED";
#[cfg(feature = "acp")]
const CSA_OWNED_ENV_KEYS: &[&str] = &[
    CSA_SESSION_ID_ENV,
    CSA_DEPTH_ENV,
    CSA_PROJECT_ROOT_ENV,
    CSA_INTERNAL_INVOCATION_ENV,
    CSA_TOOL_ENV,
    CSA_IS_SUBPROCESS_ENV,
    CSA_PARENT_TOOL_ENV,
    CSA_PARENT_SESSION_ENV,
    CSA_DAEMON_SESSION_DIR_ENV,
    CSA_FS_SANDBOXED_ENV,
    CSA_SESSION_DIR_ENV_KEY,
    CSA_PARENT_SESSION_DIR_ENV_KEY,
    csa_session::RESULT_TOML_PATH_CONTRACT_ENV,
];

#[cfg(feature = "acp")]
impl AcpTransport {
    pub(crate) fn build_system_prompt(session_config: Option<&SessionConfig>) -> Option<String> {
        let config = session_config?;
        let mut sections = Vec::new();

        if !config.no_load.is_empty() {
            sections.push(format!("No-load skills: {}", config.no_load.join(", ")));
        }
        if !config.extra_load.is_empty() {
            sections.push(format!(
                "Extra-load skills: {}",
                config.extra_load.join(", ")
            ));
        }
        if let Some(tier) = &config.tier {
            sections.push(format!("Tier: {tier}"));
        }
        if !config.models.is_empty() {
            sections.push(format!("Model candidates: {}", config.models.join(", ")));
        }
        if !config.mcp_servers.is_empty() {
            let servers = config
                .mcp_servers
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            sections.push(format!("MCP servers: {servers}"));
        }

        if sections.is_empty() {
            None
        } else {
            Some(sections.join("\n"))
        }
    }

    /// Build ACP session meta from setting_sources and MCP config.
    ///
    /// - setting sources: `{"claudeCode":{"options":{"settingSources": [...]}}}`
    /// - MCP servers: `{"claudeCode":{"options":{"mcpServers": {...}}}}`
    /// - when proxy socket exists, `mcpServers` contains a single `csa-mcp-hub` entry.
    pub(crate) fn build_session_meta(
        setting_sources: Option<&[String]>,
        session_config: Option<&SessionConfig>,
        hermes_run_config: Option<&HermesRunConfig>,
    ) -> Option<serde_json::Map<String, serde_json::Value>> {
        let mut options = serde_json::Map::new();
        if let Some(sources) = setting_sources {
            options.insert(
                "settingSources".to_string(),
                serde_json::Value::Array(
                    sources
                        .iter()
                        .map(|source| serde_json::Value::String(source.clone()))
                        .collect(),
                ),
            );
        }
        if let Some(cfg) = session_config {
            let mcp_servers = resolve_mcp_meta_servers(cfg);
            let non_empty_servers = mcp_servers
                .as_object()
                .map(|servers| !servers.is_empty())
                .unwrap_or(false);
            if non_empty_servers {
                options.insert("mcpServers".to_string(), mcp_servers);
            }
        }
        let mut meta = serde_json::Map::new();
        if !options.is_empty() {
            let mut claude_code = serde_json::Map::new();
            claude_code.insert("options".to_string(), serde_json::Value::Object(options));
            meta.insert(
                "claudeCode".to_string(),
                serde_json::Value::Object(claude_code),
            );
        }
        if let Some(hermes_options) = hermes_run_config.and_then(HermesRunConfig::meta_options) {
            let mut hermes = serde_json::Map::new();
            hermes.insert("options".to_string(), hermes_options);
            meta.insert("hermes".to_string(), serde_json::Value::Object(hermes));
        }

        (!meta.is_empty()).then_some(meta)
    }

    pub(crate) fn build_env(
        &self,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
        allow_git_push: bool,
        no_post_exec_gate: bool,
    ) -> HashMap<String, String> {
        let mut env = HashMap::new();
        if let Some(extra) = extra_env {
            env.extend(
                extra
                    .iter()
                    .filter(|(k, _)| {
                        !is_csa_owned_env_key(k) && !csa_core::env::is_startup_subtree_env_key(k)
                    })
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
        }
        csa_core::env::scrub_subtree_contract_env_map(&mut env);
        csa_core::env::strip_git_push_authorization_keys(&mut env);
        self.insert_csa_owned_env(&mut env, session);
        // Apply the trusted subtree pin LAST (after every generic merge/strip) —
        // the only writer of the pin keys in the ACP env.
        if let Some(pin) = subtree_pin {
            for (key, value) in pin.pin_env_entries() {
                env.insert(key.to_string(), value);
            }
        }
        if allow_git_push {
            env.insert(
                csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY.to_string(),
                "true".to_string(),
            );
        }

        if no_post_exec_gate {
            env.insert(
                csa_core::env::CSA_NO_POST_EXEC_GATE_ENV_KEY.to_string(),
                "1".to_string(),
            );
        }

        // Inject merge guard: prepend a `gh` wrapper to PATH that blocks
        // `gh pr merge` unless pr-bot has completed.  This is deterministic
        // environment-level enforcement — the tool subprocess cannot bypass it.
        //
        // Only applied to ACP tools (claude-code, codex) which have autonomous
        // bash execution.  Legacy tools (gemini-cli, opencode) are text-mode
        // and cannot independently call `gh pr merge`.
        csa_hooks::merge_guard::inject_merge_guard_env(&mut env);
        csa_hooks::git_guard::inject_git_guard_env(&mut env);
        if self.tool_name == "codex" {
            sanitize_env_map_for_codex(&mut env);
        }
        env
    }

    fn insert_csa_owned_env(&self, env: &mut HashMap<String, String>, session: &MetaSessionState) {
        env.insert(
            CSA_SESSION_ID_ENV.to_string(),
            session.meta_session_id.clone(),
        );
        env.insert(
            CSA_DEPTH_ENV.to_string(),
            (session.genealogy.depth + 1).to_string(),
        );
        env.insert(
            CSA_PROJECT_ROOT_ENV.to_string(),
            session.project_path.clone(),
        );
        env.insert(CSA_INTERNAL_INVOCATION_ENV.to_string(), "1".to_string());
        env.insert(CSA_TOOL_ENV.to_string(), self.tool_name.clone());
        // Mark this process as a CSA subprocess so child tools can detect
        // recursion risk (e.g. claude-code reading CLAUDE.md rules that say
        // "use csa review"). See GitHub issue #272.
        env.insert(CSA_IS_SUBPROCESS_ENV.to_string(), "1".to_string());
        if let Ok(parent_tool) = std::env::var(CSA_TOOL_ENV) {
            env.insert(CSA_PARENT_TOOL_ENV.to_string(), parent_tool);
        }
        if let Some(parent_session) = &session.genealogy.parent_session_id {
            env.insert(CSA_PARENT_SESSION_ENV.to_string(), parent_session.clone());
        }
        if std::env::var(CSA_FS_SANDBOXED_ENV).ok().as_deref() == Some("1") {
            env.insert(CSA_FS_SANDBOXED_ENV.to_string(), "1".to_string());
        }
        Self::insert_reserved_session_path_env(env, session);
    }

    fn insert_reserved_session_path_env(
        env: &mut HashMap<String, String>,
        session: &MetaSessionState,
    ) {
        let project_path = Path::new(&session.project_path);
        if let Ok(dir) =
            csa_session::manager::get_session_dir(project_path, &session.meta_session_id)
        {
            env.insert(
                CSA_SESSION_DIR_ENV_KEY.to_string(),
                dir.to_string_lossy().into_owned(),
            );
            env.insert(
                csa_session::RESULT_TOML_PATH_CONTRACT_ENV.to_string(),
                csa_session::next_turn_contract_result_path(&dir, session.turn_count)
                    .to_string_lossy()
                    .into_owned(),
            );
        } else {
            tracing::warn!("failed to compute CSA_SESSION_DIR for ACP env");
        }

        if let Some(parent_session_id) = session.genealogy.parent_session_id.as_deref() {
            match csa_session::manager::get_session_dir(project_path, parent_session_id) {
                Ok(parent_dir) => {
                    env.insert(
                        CSA_PARENT_SESSION_DIR_ENV_KEY.to_string(),
                        parent_dir.to_string_lossy().into_owned(),
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        parent_session_id,
                        error = %error,
                        "failed to compute CSA_PARENT_SESSION_DIR for ACP env"
                    );
                }
            }
        }
    }
}

#[cfg(feature = "acp")]
fn resolve_mcp_meta_servers(config: &SessionConfig) -> Value {
    if let Some(socket_path) = config
        .mcp_proxy_socket
        .as_deref()
        .map(std::path::PathBuf::from)
        && socket_path.exists()
    {
        let mut proxy_map = Map::new();
        proxy_map.insert(
            "csa-mcp-hub".to_string(),
            json!({
                "transport": "unix",
                "socketPath": socket_path,
            }),
        );
        return Value::Object(proxy_map);
    }

    let mut map = Map::new();
    for server in &config.mcp_servers {
        map.insert(
            server.name.clone(),
            json!({
                "command": server.command,
                "args": server.args,
                "env": server.env,
            }),
        );
    }
    Value::Object(map)
}

#[cfg(feature = "acp")]
fn is_csa_owned_env_key(key: &str) -> bool {
    CSA_OWNED_ENV_KEYS.contains(&key)
}

/// Attached to `anyhow::Error` context when a sandboxed ACP execution fails,
/// so callers can recover peak memory even on the error path.
#[derive(Debug, Clone)]
pub struct PeakMemoryContext(pub Option<u64>);

impl std::fmt::Display for PeakMemoryContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(mb) => write!(f, "peak_memory_mb={mb}"),
            None => write!(f, "peak_memory_mb=unknown"),
        }
    }
}

impl std::error::Error for PeakMemoryContext {}

impl PeakMemoryContext {
    /// Wrap into an `anyhow::Error` with an outer context message.
    pub fn into_anyhow(self, context: impl std::fmt::Display) -> anyhow::Error {
        anyhow::Error::new(self).context(context.to_string())
    }
}

/// Start a memory monitor given cgroup scope details and isolation plan parameters.
///
/// Shared by both ACP and legacy transport paths.  Returns `None` when
/// monitoring is not applicable (no cgroup, zero max, etc.).
pub(super) fn start_memory_monitor(
    scope_name: &str,
    pid: u32,
    isolation_plan: &IsolationPlan,
    grace_period: std::time::Duration,
    diagnostic_path: Option<PathBuf>,
) -> Option<csa_resource::memory_monitor::MemoryMonitorHandle> {
    if let Some(path) = diagnostic_path.as_deref() {
        csa_resource::memory_monitor::clear_soft_limit_diagnostic(path);
    }

    let max_mb = isolation_plan.memory_max_mb.unwrap_or(0);
    if pid == 0 || max_mb == 0 {
        return None;
    }
    let soft_pct = isolation_plan
        .soft_limit_percent
        .unwrap_or(csa_resource::memory_policy::DEFAULT_SOFT_LIMIT_PERCENT);
    let interval_secs = isolation_plan.memory_monitor_interval_seconds.unwrap_or(5);
    csa_resource::memory_monitor::start(csa_resource::memory_monitor::MemoryMonitorConfig {
        scope_name: scope_name.to_string(),
        pgid: pid as i32,
        memory_max_bytes: max_mb * 1024 * 1024,
        soft_limit_percent: soft_pct,
        interval: std::time::Duration::from_secs(interval_secs),
        grace_period,
        diagnostic_path,
    })
}

pub(super) fn memory_soft_limit_diagnostic_path(
    project_root: &Path,
    session_id: &str,
) -> Option<PathBuf> {
    if session_id.trim().is_empty() {
        return None;
    }
    csa_session::manager::get_session_dir(project_root, session_id)
        .ok()
        .and_then(|dir| {
            csa_resource::memory_monitor::soft_limit_diagnostic_path_for_session_dir(&dir)
        })
}

#[cfg(all(test, feature = "acp"))]
#[path = "transport_meta_tests.rs"]
mod tests;
