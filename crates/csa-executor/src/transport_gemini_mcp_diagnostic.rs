use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

pub(crate) const GEMINI_ALLOW_DEGRADED_MCP_ENV: &str = "CSA_GEMINI_ALLOW_DEGRADED_MCP";
const GEMINI_RUNTIME_SETTINGS_PATHS: &[&str] =
    &[".gemini/settings.json", ".config/gemini-cli/settings.json"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct McpInitDiagnostic {
    pub(crate) unhealthy_servers: Vec<String>,
    pub(crate) probe_errors: BTreeMap<String, String>,
    pub(crate) hub_reachable: bool,
}

pub(crate) fn gemini_allow_degraded_mcp(env: &HashMap<String, String>) -> bool {
    env.get(GEMINI_ALLOW_DEGRADED_MCP_ENV)
        .map(String::as_str)
        .map(|value| matches!(value, "1" | "true" | "yes" | "on"))
        .unwrap_or(true)
}

pub(crate) fn diagnose_mcp_init_failure(runtime_home: &Path) -> McpInitDiagnostic {
    let mut diagnostic = McpInitDiagnostic {
        hub_reachable: true,
        ..Default::default()
    };
    let mut seen = BTreeSet::new();

    for relative_path in GEMINI_RUNTIME_SETTINGS_PATHS {
        let settings_path = runtime_home.join(relative_path);
        let settings = load_runtime_settings(&settings_path);
        let Some(servers) = settings.get("mcpServers").and_then(Value::as_object) else {
            continue;
        };

        for (server_name, server_config) in servers {
            if !seen.insert(server_name.clone()) {
                continue;
            }
            if let Some(error) = probe_mcp_server(server_config) {
                diagnostic.unhealthy_servers.push(server_name.clone());
                diagnostic.probe_errors.insert(server_name.clone(), error);
            }
        }
    }

    diagnostic
}

pub(crate) fn disable_mcp_servers_in_runtime(
    runtime_home: &Path,
    diagnostic: &McpInitDiagnostic,
    disable_all: bool,
) -> Result<()> {
    for relative_path in GEMINI_RUNTIME_SETTINGS_PATHS {
        let settings_path = runtime_home.join(relative_path);
        if !settings_path.exists() {
            continue;
        }

        let mut settings = load_runtime_settings(&settings_path);
        let Some(servers) = settings
            .get_mut("mcpServers")
            .and_then(serde_json::Value::as_object_mut)
        else {
            continue;
        };

        if disable_all {
            servers.clear();
        } else {
            for server_name in &diagnostic.unhealthy_servers {
                servers.remove(server_name);
            }
        }

        let serialized = serde_json::to_string_pretty(&settings)
            .context("failed to serialize degraded gemini runtime settings")?;
        fs::write(&settings_path, format!("{serialized}\n")).with_context(|| {
            format!(
                "failed to write degraded gemini runtime settings {}",
                settings_path.display()
            )
        })?;
    }

    Ok(())
}

pub(crate) fn format_mcp_init_warning_summary(
    diagnostic: &McpInitDiagnostic,
    retried_without_all_servers: bool,
) -> String {
    let server_list = if diagnostic.unhealthy_servers.is_empty() {
        "unknown server".to_string()
    } else {
        diagnostic.unhealthy_servers.join(", ")
    };
    let mode_note = if retried_without_all_servers {
        "disabled all configured MCP servers after gemini-cli startup refusal"
    } else {
        "disabled unhealthy MCP servers and continued with degraded MCP"
    };
    format!(
        "gemini-cli MCP init degraded ({server_list}) — {mode_note}; \
retry with --force-ignore-tier-setting + different --tool, or run 'csa doctor' to diagnose"
    )
}

fn probe_mcp_server(server_config: &Value) -> Option<String> {
    let command = server_config.get("command").and_then(Value::as_str)?;
    if command.contains(std::path::MAIN_SEPARATOR) {
        let path = Path::new(command);
        if !path.exists() {
            return Some(format!("command path missing: {}", path.display()));
        }
        return None;
    }

    if resolve_command_path(command).is_none() {
        return Some(format!("command not found on PATH: {command}"));
    }

    None
}

fn resolve_command_path(command: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for entry in std::env::split_paths(&path_var) {
        let candidate = entry.join(command);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn load_runtime_settings(settings_path: &Path) -> Value {
    let Ok(raw) = fs::read_to_string(settings_path) else {
        return Value::Object(Default::default());
    };

    serde_json::from_str(&raw).unwrap_or_else(|_| Value::Object(Default::default()))
}
