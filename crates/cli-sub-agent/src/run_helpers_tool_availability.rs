use std::borrow::Cow;
use std::collections::HashMap;

use csa_config::{ProjectConfig, TransportKind, default_transport_for_tool};
use csa_executor::{ClaudeCodeTransport, CodexTransport, install_hint_for_known_tool};

const OPENAI_COMPAT_BASE_URL_ENV: &str = "OPENAI_COMPAT_BASE_URL";
const OPENAI_COMPAT_API_KEY_ENV: &str = "OPENAI_COMPAT_API_KEY";
const OPENAI_COMPAT_MODEL_ENV: &str = "OPENAI_COMPAT_MODEL";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolBinaryAvailability {
    Available {
        binary_name: String,
    },
    Missing {
        binary_name: String,
        hint: Cow<'static, str>,
    },
}

impl ToolBinaryAvailability {
    pub(crate) fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

#[cfg(test)]
fn assume_tool_binaries_available_for_tests() -> bool {
    [
        super::TEST_SKIP_TOOL_AVAILABILITY_CHECK_ENV,
        super::TEST_ASSUME_TOOLS_AVAILABLE_ENV,
    ]
    .into_iter()
    .any(|env_name| {
        std::env::var(env_name)
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

pub(crate) fn resolved_codex_transport(config: Option<&ProjectConfig>) -> CodexTransport {
    config
        .and_then(|cfg| cfg.tool_transport("codex"))
        // No per-tool transport configured: mirror the EXECUTION-time routing
        // default (`default_transport_for_tool` = Cli), NOT the metadata build
        // default (`CodexTransport::default_for_build` = Acp). The two diverge —
        // `TransportFactory::mode_for_executor` routes codex through the `codex`
        // CLI, while the build default names the `codex-acp` binary. Probing
        // `codex-acp` for availability is a false-negative that silently drops
        // codex from tier failover (#1714).
        .or_else(|| default_transport_for_tool("codex"))
        .map(|transport| match transport {
            TransportKind::Cli => CodexTransport::Cli,
            TransportKind::Acp => CodexTransport::Acp,
            TransportKind::Auto => unreachable!("resolved transports never include Auto"),
            TransportKind::Tmux => unreachable!("codex does not support tmux transport"),
        })
        .unwrap_or(CodexTransport::Cli)
}

pub(crate) fn resolved_claude_code_transport(
    config: Option<&ProjectConfig>,
) -> ClaudeCodeTransport {
    config
        .and_then(|cfg| cfg.tool_transport("claude-code"))
        // See `resolved_codex_transport`: fall back to the execution-time routing
        // default (Cli), not the metadata build default (Acp = `claude-code-acp`),
        // so availability probing matches how the tool actually runs (#1714).
        .or_else(|| default_transport_for_tool("claude-code"))
        .map(|transport| match transport {
            TransportKind::Cli => ClaudeCodeTransport::Cli,
            TransportKind::Acp => ClaudeCodeTransport::Acp,
            TransportKind::Tmux => ClaudeCodeTransport::Tmux,
            TransportKind::Auto => unreachable!("resolved transports never include Auto"),
        })
        .unwrap_or(ClaudeCodeTransport::Cli)
}

pub(crate) fn resolved_tool_binary_name(
    tool_name: &str,
    config: Option<&ProjectConfig>,
) -> Option<&'static str> {
    match tool_name {
        "gemini-cli" => Some("gemini"),
        "opencode" => Some("opencode"),
        "codex" => Some(resolved_codex_transport(config).runtime_binary_name()),
        "claude-code" => Some(resolved_claude_code_transport(config).runtime_binary_name()),
        "antigravity-cli" => Some("antigravity"),
        "openai-compat" => None,
        _ => None,
    }
}

fn env_is_set(extra_env: Option<&HashMap<String, String>>, key: &str) -> bool {
    if let Some(value) = extra_env.and_then(|env| env.get(key)) {
        return !value.trim().is_empty();
    }

    std::env::var(key)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn configured_openai_compat_field(
    config: Option<&ProjectConfig>,
    extra_env: Option<&HashMap<String, String>>,
    get_field: impl Fn(&csa_config::ToolConfig) -> Option<&String>,
    env_key: &str,
) -> bool {
    config
        .and_then(|cfg| cfg.tools.get("openai-compat"))
        .and_then(get_field)
        .is_some_and(|value| !value.trim().is_empty())
        || env_is_set(extra_env, env_key)
}

fn openai_compat_model_configured(
    config: Option<&ProjectConfig>,
    extra_env: Option<&HashMap<String, String>>,
    model_hint: Option<&str>,
) -> bool {
    model_hint.is_some_and(|value| !value.trim().is_empty())
        || config
            .and_then(|cfg| cfg.tool_default_model("openai-compat"))
            .is_some_and(|value| !value.trim().is_empty())
        || env_is_set(extra_env, OPENAI_COMPAT_MODEL_ENV)
}

fn openai_compat_availability(
    config: Option<&ProjectConfig>,
    extra_env: Option<&HashMap<String, String>>,
    model_hint: Option<&str>,
) -> ToolBinaryAvailability {
    let mut missing = Vec::new();
    if !configured_openai_compat_field(
        config,
        extra_env,
        |tool| tool.base_url.as_ref(),
        OPENAI_COMPAT_BASE_URL_ENV,
    ) {
        missing.push("base_url");
    }
    if !configured_openai_compat_field(
        config,
        extra_env,
        |tool| tool.api_key.as_ref(),
        OPENAI_COMPAT_API_KEY_ENV,
    ) {
        missing.push("api_key");
    }
    if !openai_compat_model_configured(config, extra_env, model_hint) {
        missing.push("model");
    }

    if missing.is_empty() {
        return ToolBinaryAvailability::Available {
            binary_name: "openai-compat".to_string(),
        };
    }

    ToolBinaryAvailability::Missing {
        binary_name: "openai-compat".to_string(),
        hint: Cow::Owned(format!(
            "{}; missing {}",
            install_hint_for_known_tool("openai-compat")
                .unwrap_or("Configure openai-compat before use"),
            missing.join(", ")
        )),
    }
}

#[cfg(test)]
pub(crate) fn tool_runtime_availability(
    tool_name: &str,
    config: Option<&ProjectConfig>,
    model_hint: Option<&str>,
) -> ToolBinaryAvailability {
    tool_runtime_availability_with_env(tool_name, config, model_hint, None)
}

pub(crate) fn tool_runtime_availability_with_env(
    tool_name: &str,
    config: Option<&ProjectConfig>,
    model_hint: Option<&str>,
    extra_env: Option<&HashMap<String, String>>,
) -> ToolBinaryAvailability {
    if tool_name == "openai-compat" {
        return openai_compat_availability(config, extra_env, model_hint);
    }

    tool_binary_availability(tool_name, config)
}

pub(crate) fn tool_binary_availability(
    tool_name: &str,
    config: Option<&ProjectConfig>,
) -> ToolBinaryAvailability {
    if tool_name == "openai-compat" {
        return openai_compat_availability(config, None, None);
    }

    #[cfg(test)]
    if assume_tool_binaries_available_for_tests() {
        return ToolBinaryAvailability::Available {
            binary_name: resolved_tool_binary_name(tool_name, config)
                .unwrap_or(tool_name)
                .to_string(),
        };
    }

    let Some(binary_name) = resolved_tool_binary_name(tool_name, config) else {
        return ToolBinaryAvailability::Missing {
            binary_name: tool_name.to_string(),
            hint: Cow::Borrowed("Unknown tool"),
        };
    };

    let installed = std::process::Command::new("which")
        .arg(binary_name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if installed {
        ToolBinaryAvailability::Available {
            binary_name: binary_name.to_string(),
        }
    } else {
        let hint = match tool_name {
            "codex" => Cow::Borrowed(resolved_codex_transport(config).install_hint()),
            "claude-code" => Cow::Borrowed(resolved_claude_code_transport(config).install_hint()),
            _ => Cow::Borrowed(
                install_hint_for_known_tool(tool_name)
                    .unwrap_or("Install the tool and ensure it is on PATH"),
            ),
        };
        ToolBinaryAvailability::Missing {
            binary_name: binary_name.to_string(),
            hint,
        }
    }
}

pub(crate) fn is_tool_binary_available_for_config(
    tool_name: &str,
    config: Option<&ProjectConfig>,
) -> bool {
    tool_binary_availability(tool_name, config).is_available()
}

pub(crate) fn is_tool_runtime_available_for_config_with_env(
    tool_name: &str,
    config: Option<&ProjectConfig>,
    model_hint: Option<&str>,
    extra_env: Option<&HashMap<String, String>>,
) -> bool {
    tool_runtime_availability_with_env(tool_name, config, model_hint, extra_env).is_available()
}

#[cfg(test)]
mod failover_detection_tests {
    use super::*;
    use std::collections::HashMap;

    use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig};

    fn project_config_with_openai(tool_config: ToolConfig) -> ProjectConfig {
        let mut tools = HashMap::new();
        tools.insert("openai-compat".to_string(), tool_config);

        ProjectConfig {
            schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: chrono::Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            acp: Default::default(),
            tools,
            review: None,
            debate: None,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
            tool_aliases: HashMap::new(),
            preferences: None,
            github: None,
            session: Default::default(),
            memory: Default::default(),
            hooks: Default::default(),
            run: Default::default(),
            execution: Default::default(),
            session_wait: None,
            preflight: Default::default(),
            vcs: Default::default(),
            filesystem_sandbox: Default::default(),
        }
    }

    // Regression for #1714: with no project config, availability probing MUST
    // mirror execution-time transport routing (`default_transport_for_tool` →
    // CLI), not the metadata build default (`default_for_build` → ACP). Probing
    // the ACP binary names ("codex-acp" / "claude-code-acp") when execution
    // actually uses the CLI binaries is a false-negative availability miss that
    // silently drops codex/claude-code from tier failover candidate lists.
    #[test]
    fn resolved_codex_transport_defaults_to_cli_without_config() {
        assert_eq!(resolved_codex_transport(None), CodexTransport::Cli);
    }

    #[test]
    fn resolved_claude_code_transport_defaults_to_cli_without_config() {
        assert_eq!(
            resolved_claude_code_transport(None),
            ClaudeCodeTransport::Cli
        );
    }

    #[test]
    fn resolved_binary_name_uses_cli_default_when_config_absent() {
        // Before the fix these returned "codex-acp" / "claude-code-acp".
        assert_eq!(resolved_tool_binary_name("codex", None), Some("codex"));
        assert_eq!(
            resolved_tool_binary_name("claude-code", None),
            Some("claude")
        );
    }

    #[test]
    fn openai_compat_without_http_config_is_missing() {
        let _lock = crate::test_env_lock::TEST_ENV_LOCK
            .clone()
            .blocking_lock_owned();
        let _base = crate::test_env_lock::ScopedEnvVarRestore::unset(OPENAI_COMPAT_BASE_URL_ENV);
        let _key = crate::test_env_lock::ScopedEnvVarRestore::unset(OPENAI_COMPAT_API_KEY_ENV);
        let _model = crate::test_env_lock::ScopedEnvVarRestore::unset(OPENAI_COMPAT_MODEL_ENV);

        let availability = tool_runtime_availability(
            "openai-compat",
            None,
            Some("openai-compat/openai/gpt-5/high"),
        );

        assert!(matches!(
            availability,
            ToolBinaryAvailability::Missing { .. }
        ));
    }

    #[test]
    fn openai_compat_tier_model_with_http_config_is_available() {
        let _lock = crate::test_env_lock::TEST_ENV_LOCK
            .clone()
            .blocking_lock_owned();
        let _base = crate::test_env_lock::ScopedEnvVarRestore::unset(OPENAI_COMPAT_BASE_URL_ENV);
        let _key = crate::test_env_lock::ScopedEnvVarRestore::unset(OPENAI_COMPAT_API_KEY_ENV);
        let _model = crate::test_env_lock::ScopedEnvVarRestore::unset(OPENAI_COMPAT_MODEL_ENV);
        let config = project_config_with_openai(ToolConfig {
            base_url: Some("http://localhost:8317".to_string()),
            api_key: Some("test-key".to_string()),
            ..Default::default()
        });

        let availability = tool_runtime_availability(
            "openai-compat",
            Some(&config),
            Some("openai-compat/openai/gpt-5/high"),
        );

        assert!(availability.is_available());
    }

    #[test]
    fn openai_compat_global_tool_env_is_available_without_project_http_config() {
        let _lock = crate::test_env_lock::TEST_ENV_LOCK
            .clone()
            .blocking_lock_owned();
        let _base = crate::test_env_lock::ScopedEnvVarRestore::unset(OPENAI_COMPAT_BASE_URL_ENV);
        let _key = crate::test_env_lock::ScopedEnvVarRestore::unset(OPENAI_COMPAT_API_KEY_ENV);
        let _model = crate::test_env_lock::ScopedEnvVarRestore::unset(OPENAI_COMPAT_MODEL_ENV);
        let extra_env = HashMap::from([
            (
                OPENAI_COMPAT_BASE_URL_ENV.to_string(),
                "http://localhost:8317".to_string(),
            ),
            (
                OPENAI_COMPAT_API_KEY_ENV.to_string(),
                "test-key".to_string(),
            ),
            (
                OPENAI_COMPAT_MODEL_ENV.to_string(),
                "local-model".to_string(),
            ),
        ]);

        let availability =
            tool_runtime_availability_with_env("openai-compat", None, None, Some(&extra_env));

        assert!(availability.is_available());
    }

    #[test]
    fn openai_compat_without_model_hint_requires_default_model() {
        let _lock = crate::test_env_lock::TEST_ENV_LOCK
            .clone()
            .blocking_lock_owned();
        let _base = crate::test_env_lock::ScopedEnvVarRestore::unset(OPENAI_COMPAT_BASE_URL_ENV);
        let _key = crate::test_env_lock::ScopedEnvVarRestore::unset(OPENAI_COMPAT_API_KEY_ENV);
        let _model = crate::test_env_lock::ScopedEnvVarRestore::unset(OPENAI_COMPAT_MODEL_ENV);
        let config = project_config_with_openai(ToolConfig {
            base_url: Some("http://localhost:8317".to_string()),
            api_key: Some("test-key".to_string()),
            ..Default::default()
        });

        let availability = tool_runtime_availability("openai-compat", Some(&config), None);

        assert!(matches!(
            availability,
            ToolBinaryAvailability::Missing { .. }
        ));
    }
}
