use std::borrow::Cow;

use csa_config::{ProjectConfig, ToolTransport};
use csa_executor::{CodexTransport, install_hint_for_known_tool};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolBinaryAvailability {
    Available {
        binary_name: String,
    },
    Missing {
        binary_name: String,
        hint: Cow<'static, str>,
    },
    Unsupported {
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
        .map(|transport| match transport {
            ToolTransport::Cli => CodexTransport::Cli,
            ToolTransport::Acp => CodexTransport::Acp,
        })
        .unwrap_or_else(CodexTransport::default_for_build)
}

pub(crate) fn resolved_tool_binary_name(
    tool_name: &str,
    config: Option<&ProjectConfig>,
) -> Option<&'static str> {
    match tool_name {
        "gemini-cli" => Some("gemini"),
        "opencode" => Some("opencode"),
        "codex" => Some(resolved_codex_transport(config).runtime_binary_name()),
        "claude-code" => Some("claude-code-acp"),
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

    if tool_name == "codex"
        && matches!(resolved_codex_transport(config), CodexTransport::Acp)
        && !csa_executor::CodexRuntimeMetadata::acp_compiled_in()
    {
        return ToolBinaryAvailability::Unsupported {
            binary_name: binary_name.to_string(),
            hint: Cow::Borrowed(
                "Rebuild CSA with `cargo build --features codex-acp`, or remove `[tools.codex].transport = \"acp\"`.",
            ),
        };
    }

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
        let hint = if tool_name == "codex" {
            Cow::Borrowed(resolved_codex_transport(config).install_hint())
        } else {
            Cow::Borrowed(
                install_hint_for_known_tool(tool_name)
                    .unwrap_or("Install the tool and ensure it is on PATH"),
            )
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
