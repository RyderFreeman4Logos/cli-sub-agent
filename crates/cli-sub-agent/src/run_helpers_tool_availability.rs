use std::borrow::Cow;

use csa_config::{ProjectConfig, TransportKind, default_transport_for_tool};
use csa_executor::{ClaudeCodeTransport, CodexTransport, install_hint_for_known_tool};

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

pub(crate) fn tool_binary_availability(
    tool_name: &str,
    config: Option<&ProjectConfig>,
) -> ToolBinaryAvailability {
    #[cfg(test)]
    if assume_tool_binaries_available_for_tests() {
        return ToolBinaryAvailability::Available {
            binary_name: resolved_tool_binary_name(tool_name, config)
                .unwrap_or(tool_name)
                .to_string(),
        };
    }

    if tool_name == "openai-compat" {
        return ToolBinaryAvailability::Available {
            binary_name: "openai-compat".to_string(),
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

#[cfg(test)]
mod failover_detection_tests {
    use super::*;

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
}
