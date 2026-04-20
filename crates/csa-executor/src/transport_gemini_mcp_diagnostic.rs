use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
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

pub(crate) fn diagnose_mcp_init_failure(
    runtime_home: &Path,
    path_override: Option<&OsStr>,
) -> McpInitDiagnostic {
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
            if let Some(error) = probe_mcp_server(server_config, path_override) {
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

fn probe_mcp_server(server_config: &Value, path_override: Option<&OsStr>) -> Option<String> {
    let command = server_config.get("command").and_then(Value::as_str)?;
    if command.contains(std::path::MAIN_SEPARATOR) {
        let path = Path::new(command);
        if !path.exists() {
            return Some(format!("command path missing: {}", path.display()));
        }
        return None;
    }

    if resolve_command_path(command, path_override).is_none() {
        return Some(format!("command not found on PATH: {command}"));
    }

    None
}

fn resolve_command_path(command: &str, path_override: Option<&OsStr>) -> Option<PathBuf> {
    let path_var = path_override
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var_os("PATH"))?;
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

#[cfg(test)]
mod tests {
    use super::diagnose_mcp_init_failure;
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    static PATH_ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

    struct ScopedPathEnv {
        original: Option<OsString>,
    }

    impl ScopedPathEnv {
        fn set(value: &OsString) -> Self {
            let original = std::env::var_os("PATH");
            // SAFETY: test-scoped env mutation guarded by PATH_ENV_LOCK.
            unsafe { std::env::set_var("PATH", value) };
            Self { original }
        }
    }

    impl Drop for ScopedPathEnv {
        fn drop(&mut self) {
            // SAFETY: test-scoped env mutation guarded by PATH_ENV_LOCK.
            unsafe {
                match self.original.take() {
                    Some(value) => std::env::set_var("PATH", value),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
    }

    fn write_runtime_settings(runtime_home: &Path, command: &str) {
        let gemini_dir = runtime_home.join(".gemini");
        fs::create_dir_all(&gemini_dir).expect("create .gemini");
        fs::write(
            gemini_dir.join("settings.json"),
            format!(
                r#"{{
  "mcpServers": {{
    "fake-mcp": {{
      "command": "{command}",
      "args": ["--mcp"]
    }}
  }}
}}"#
            ),
        )
        .expect("write settings");
    }

    fn write_executable(dir: &Path, name: &str) -> PathBuf {
        let script_path = dir.join(name);
        fs::write(&script_path, "#!/bin/sh\nexit 0\n").expect("write fake executable");
        let mut perms = fs::metadata(&script_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).expect("chmod +x");
        script_path
    }

    #[test]
    fn diagnose_mcp_init_failure_probe_respects_prepared_path_over_parent_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_runtime_settings(temp.path(), "fake-mcp-server");

        let prepared_bin = temp.path().join("prepared-bin");
        fs::create_dir_all(&prepared_bin).expect("create prepared bin");
        write_executable(&prepared_bin, "fake-mcp-server");

        let prepared_path = prepared_bin.into_os_string();
        let diagnostic = diagnose_mcp_init_failure(temp.path(), Some(prepared_path.as_os_str()));

        assert!(
            diagnostic.probe_errors.is_empty(),
            "expected prepared PATH command to resolve, got: {:?}",
            diagnostic.probe_errors
        );
        assert!(
            diagnostic.unhealthy_servers.is_empty(),
            "expected no unhealthy servers, got: {:?}",
            diagnostic.unhealthy_servers
        );
    }

    #[test]
    fn diagnose_mcp_init_failure_probe_without_override_falls_back_to_parent_path() {
        let _env_lock = PATH_ENV_LOCK.lock().expect("PATH env lock poisoned");
        let temp = tempfile::tempdir().expect("tempdir");
        write_runtime_settings(temp.path(), "fake-mcp-server");

        let parent_bin = temp.path().join("parent-bin");
        fs::create_dir_all(&parent_bin).expect("create parent bin");
        write_executable(&parent_bin, "fake-mcp-server");

        let mut joined = OsString::from(parent_bin.as_os_str());
        if let Some(existing) = std::env::var_os("PATH") {
            joined.push(OsString::from(":"));
            joined.push(existing);
        }
        let _path = ScopedPathEnv::set(&joined);

        let diagnostic = diagnose_mcp_init_failure(temp.path(), None);

        assert!(
            diagnostic.probe_errors.is_empty(),
            "expected parent PATH fallback to resolve command, got: {:?}",
            diagnostic.probe_errors
        );
        assert!(
            diagnostic.unhealthy_servers.is_empty(),
            "expected no unhealthy servers, got: {:?}",
            diagnostic.unhealthy_servers
        );
    }

    #[test]
    fn diagnose_mcp_init_failure_prepared_path_hides_parent_path_only_command() {
        let _env_lock = PATH_ENV_LOCK.lock().expect("PATH env lock poisoned");
        let temp = tempfile::tempdir().expect("tempdir");
        write_runtime_settings(temp.path(), "fake-mcp-server");

        let parent_bin = temp.path().join("parent-bin");
        fs::create_dir_all(&parent_bin).expect("create parent bin");
        write_executable(&parent_bin, "fake-mcp-server");

        let prepared_bin = temp.path().join("prepared-bin");
        fs::create_dir_all(&prepared_bin).expect("create prepared bin");

        let mut joined = OsString::from(parent_bin.as_os_str());
        if let Some(existing) = std::env::var_os("PATH") {
            joined.push(OsString::from(":"));
            joined.push(existing);
        }
        let _path = ScopedPathEnv::set(&joined);

        let prepared_path = prepared_bin.into_os_string();
        let diagnostic = diagnose_mcp_init_failure(temp.path(), Some(prepared_path.as_os_str()));

        assert_eq!(diagnostic.unhealthy_servers, vec!["fake-mcp".to_string()]);
        assert_eq!(
            diagnostic.probe_errors.get("fake-mcp").map(String::as_str),
            Some("command not found on PATH: fake-mcp-server")
        );
    }
}
