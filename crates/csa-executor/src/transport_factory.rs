use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::executor::Executor;
use csa_acp::SessionConfig;

use super::{AcpTransport, LegacyTransport, Transport, TransportCapabilities};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    #[serde(rename = "cli")]
    Legacy,
    Acp,
    OpenaiCompat,
}

#[derive(Debug, thiserror::Error)]
pub enum TransportFactoryError {
    #[error("transport `{requested}` not supported for executor `{executor}`: {reason}")]
    UnsupportedTransport {
        requested: TransportMode,
        executor: String,
        reason: &'static str,
    },
}

pub struct TransportFactory;

impl std::fmt::Display for TransportMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Legacy => f.write_str("cli"),
            Self::Acp => f.write_str("acp"),
            Self::OpenaiCompat => f.write_str("openai_compat"),
        }
    }
}

impl TransportMode {
    /// Return the informational capabilities for this transport mode.
    pub fn capabilities(self) -> TransportCapabilities {
        match self {
            Self::Legacy => TransportCapabilities {
                streaming: false,
                session_resume: true,
                session_fork: false,
                typed_events: false,
            },
            Self::Acp => TransportCapabilities {
                streaming: true,
                session_resume: true,
                // session_fork depends on the specific tool (claude-code: true,
                // codex: only with codex-pty-fork feature, others: false).
                // Without tool context, default to false; concrete AcpTransport
                // instances return the accurate tool-aware value.
                session_fork: false,
                typed_events: true,
            },
            Self::OpenaiCompat => TransportCapabilities {
                streaming: false,
                session_resume: false,
                session_fork: false,
                typed_events: false,
            },
        }
    }
}

impl TransportFactory {
    pub fn mode_for_tool(tool_name: &str) -> TransportMode {
        match tool_name {
            "claude-code" => TransportMode::Acp,
            "openai-compat" => TransportMode::OpenaiCompat,
            _ => TransportMode::Legacy,
        }
    }

    fn mode_for_executor(executor: &Executor) -> Result<TransportMode> {
        match executor {
            Executor::Codex { .. } => match executor.codex_transport() {
                Some(crate::CodexTransport::Cli) | None => Ok(TransportMode::Legacy),
                Some(crate::CodexTransport::Acp) => {
                    Self::validate_mode_for_executor(executor, TransportMode::Acp)?;
                    Ok(TransportMode::Acp)
                }
            },
            Executor::ClaudeCode { .. } => match executor.claude_code_transport() {
                Some(crate::ClaudeCodeTransport::Cli) => {
                    Self::validate_mode_for_executor(executor, TransportMode::Legacy)?;
                    Ok(TransportMode::Legacy)
                }
                Some(crate::ClaudeCodeTransport::Acp) | None => {
                    Self::validate_mode_for_executor(executor, TransportMode::Acp)?;
                    Ok(TransportMode::Acp)
                }
            },
            _ => {
                let mode = Self::mode_for_tool(executor.tool_name());
                Self::validate_mode_for_executor(executor, mode)?;
                Ok(mode)
            }
        }
    }

    /// Validate that `mode` is supported for `executor`. Returns
    /// `TransportFactoryError::UnsupportedTransport` on unsupported combos.
    ///
    /// Compatibility matrix (source of truth):
    ///
    /// | Executor     | Legacy | Acp                      | OpenaiCompat |
    /// |--------------|--------|--------------------------| -------------|
    /// | ClaudeCode   | Yes    | Yes                      | No           |
    /// | Codex        | Yes    | Yes                      | No           |
    /// | GeminiCli    | Yes    | Yes                      | No           |
    /// | Opencode     | Yes    | No                       | No           |
    /// | OpenaiCompat | No     | No                       | Yes          |
    fn validate_mode_for_executor(
        executor: &Executor,
        mode: TransportMode,
    ) -> Result<(), TransportFactoryError> {
        let err = |reason: &'static str| {
            Err(TransportFactoryError::UnsupportedTransport {
                requested: mode,
                executor: executor.tool_name().to_string(),
                reason,
            })
        };

        match (executor, mode) {
            // ClaudeCode: Legacy CLI and ACP are both supported
            (Executor::ClaudeCode { .. }, TransportMode::Acp) => Ok(()),
            (Executor::ClaudeCode { .. }, TransportMode::Legacy) => Ok(()),
            (Executor::ClaudeCode { .. }, TransportMode::OpenaiCompat) => {
                err("claude-code only supports Legacy or ACP transport")
            }

            // Codex: Legacy and ACP are both supported
            (Executor::Codex { .. }, TransportMode::Legacy) => Ok(()),
            (Executor::Codex { .. }, TransportMode::Acp) => Ok(()),
            (Executor::Codex { .. }, TransportMode::OpenaiCompat) => {
                err("codex only supports Legacy or ACP transport")
            }

            // GeminiCli: Legacy and ACP (native --acp mode)
            (Executor::GeminiCli { .. }, TransportMode::Legacy) => Ok(()),
            (Executor::GeminiCli { .. }, TransportMode::Acp) => Ok(()),
            (Executor::GeminiCli { .. }, TransportMode::OpenaiCompat) => {
                err("gemini-cli only supports Legacy or ACP transport")
            }

            // Opencode: Legacy only
            (Executor::Opencode { .. }, TransportMode::Legacy) => Ok(()),
            (Executor::Opencode { .. }, TransportMode::Acp) => err("opencode has no ACP transport"),
            (Executor::Opencode { .. }, TransportMode::OpenaiCompat) => {
                err("opencode only supports Legacy transport")
            }

            // OpenaiCompat: OpenaiCompat mode only
            (Executor::OpenaiCompat { .. }, TransportMode::OpenaiCompat) => Ok(()),
            (Executor::OpenaiCompat { .. }, TransportMode::Legacy) => {
                err("openai-compat has no CLI binary")
            }
            (Executor::OpenaiCompat { .. }, TransportMode::Acp) => {
                err("openai-compat does not support ACP transport")
            }
        }
    }

    /// Create a transport by explicitly specifying the mode.
    ///
    /// Phase 2 will use this to honor `[tools.<name>].transport` config.
    pub fn create_with_mode(
        executor: &Executor,
        mode: TransportMode,
        session_config: Option<SessionConfig>,
    ) -> Result<Box<dyn Transport>> {
        Self::validate_mode_for_executor(executor, mode)?;
        Self::instantiate(executor, mode, session_config)
    }

    /// Create a transport by auto-inferring mode from the executor.
    pub fn create(
        executor: &Executor,
        session_config: Option<SessionConfig>,
    ) -> Result<Box<dyn Transport>> {
        let mode = Self::mode_for_executor(executor)?;
        Self::instantiate(executor, mode, session_config)
    }

    fn instantiate(
        executor: &Executor,
        mode: TransportMode,
        session_config: Option<SessionConfig>,
    ) -> Result<Box<dyn Transport>> {
        match mode {
            TransportMode::Legacy => Ok(Box::new(LegacyTransport::new(executor.clone()))),
            TransportMode::Acp => Ok(Box::new(AcpTransport::new(
                executor.tool_name(),
                session_config,
            ))),
            TransportMode::OpenaiCompat => {
                let default_model = if let Executor::OpenaiCompat { model_override, .. } = executor
                {
                    model_override.clone()
                } else {
                    None
                };
                Ok(Box::new(
                    crate::transport_openai_compat::OpenaiCompatTransport::new(default_model),
                ))
            }
        }
    }

    pub fn create_openai_compat(
        config: crate::transport_openai_compat::OpenaiCompatConfig,
    ) -> Box<dyn Transport> {
        Box::new(crate::transport_openai_compat::OpenaiCompatTransport::with_config(config))
    }
}
