use anyhow::Result;
use serde::{Deserialize, Serialize};

#[cfg(feature = "acp")]
use super::AcpTransport;
use super::{ClaudeCodeCliTransport, LegacyTransport, Transport, TransportCapabilities};
use crate::executor::Executor;
use crate::session_config::SessionConfig;

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
            // ACP transport for codex is gated behind the `codex-acp` cargo
            // feature (disabled by default) in favour of native CLI resume
            // (`codex exec resume <id>`, codex-cli 0.125.0+, #760 / #1128).
            // Only an explicit `Some(Acp)` request attempts ACP; `None` and
            // `Some(Cli)` both route through the CLI transport.
            Executor::Codex { .. } => match executor.codex_transport() {
                Some(crate::CodexTransport::Acp) => {
                    Self::validate_mode_for_executor(executor, TransportMode::Acp)?;
                    Ok(TransportMode::Acp)
                }
                Some(crate::CodexTransport::Cli) | None => {
                    Self::validate_mode_for_executor(executor, TransportMode::Legacy)?;
                    Ok(TransportMode::Legacy)
                }
            },
            Executor::ClaudeCode { .. } => match executor.claude_code_transport() {
                // ACP transport for claude-code is gated behind the `claude-code-acp` cargo
                // feature (disabled by default) due to startup-crash bugs #1115/#1117.
                // Only an explicit `Some(Acp)` request attempts ACP; `None` and `Some(Cli)`
                // both route through the CLI transport.
                Some(crate::ClaudeCodeTransport::Acp) => {
                    Self::validate_mode_for_executor(executor, TransportMode::Acp)?;
                    Ok(TransportMode::Acp)
                }
                Some(crate::ClaudeCodeTransport::Cli) | None => {
                    Self::validate_mode_for_executor(executor, TransportMode::Legacy)?;
                    Ok(TransportMode::Legacy)
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
    /// | Executor     | Legacy | Acp                                      | OpenaiCompat |
    /// |--------------|--------|------------------------------------------| -------------|
    /// | ClaudeCode   | Yes    | Yes (requires `claude-code-acp` feature) | No           |
    /// | Codex        | Yes    | Yes (requires `codex-acp` feature)       | No           |
    /// | GeminiCli    | Yes    | Yes                                      | No           |
    /// | Opencode     | Yes    | No                                       | No           |
    /// | OpenaiCompat | No     | No                                       | Yes          |
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
            #[cfg(not(feature = "acp"))]
            (_, TransportMode::Acp) => err(
                "ACP transport requires the `acp` cargo feature; rebuild with `--features acp` to enable",
            ),

            // ClaudeCode + ACP: gated behind the `claude-code-acp` cargo feature.
            // ACP for claude-code crashes at session startup (turn_count=0, #1115/#1117).
            // Build with `--features claude-code-acp` to re-enable this path.
            #[cfg(all(feature = "acp", not(feature = "claude-code-acp")))]
            (Executor::ClaudeCode { .. }, TransportMode::Acp) => err(
                "claude-code ACP transport requires the `claude-code-acp` cargo feature \
                     (disabled by default due to startup-crash bugs #1115/#1117); \
                     rebuild with `--features claude-code-acp` to enable",
            ),
            #[cfg(all(feature = "acp", feature = "claude-code-acp"))]
            (Executor::ClaudeCode { .. }, TransportMode::Acp) => Ok(()),
            (Executor::ClaudeCode { .. }, TransportMode::Legacy) => Ok(()),
            (Executor::ClaudeCode { .. }, TransportMode::OpenaiCompat) => {
                err("claude-code only supports cli or acp transport")
            }

            // Codex + ACP: gated behind the `codex-acp` cargo feature.
            // Native CLI resume (`codex exec resume <id>`, codex-cli 0.125.0+)
            // is the default and is empirically equivalent to ACP loadSession
            // on server-side cache reuse (#760 / #1128).
            // Build with `--features codex-acp` to re-enable this path.
            (Executor::Codex { .. }, TransportMode::Legacy) => Ok(()),
            #[cfg(all(feature = "acp", not(feature = "codex-acp")))]
            (Executor::Codex { .. }, TransportMode::Acp) => err(
                "codex ACP transport requires the `codex-acp` cargo feature \
                     (disabled by default in favour of native `codex exec resume <id>`, \
                     #760/#1128); rebuild with `--features codex-acp` to enable",
            ),
            #[cfg(all(feature = "acp", feature = "codex-acp"))]
            (Executor::Codex { .. }, TransportMode::Acp) => Ok(()),
            (Executor::Codex { .. }, TransportMode::OpenaiCompat) => {
                err("codex only supports cli or acp transport")
            }

            // GeminiCli: Legacy and ACP (native --acp mode)
            (Executor::GeminiCli { .. }, TransportMode::Legacy) => Ok(()),
            #[cfg(feature = "acp")]
            (Executor::GeminiCli { .. }, TransportMode::Acp) => Ok(()),
            (Executor::GeminiCli { .. }, TransportMode::OpenaiCompat) => {
                err("gemini-cli only supports cli or acp transport")
            }

            // Opencode: Legacy only
            (Executor::Opencode { .. }, TransportMode::Legacy) => Ok(()),
            #[cfg(feature = "acp")]
            (Executor::Opencode { .. }, TransportMode::Acp) => err("opencode has no acp transport"),
            (Executor::Opencode { .. }, TransportMode::OpenaiCompat) => {
                err("opencode only supports cli transport")
            }

            // OpenaiCompat: OpenaiCompat mode only
            (Executor::OpenaiCompat { .. }, TransportMode::OpenaiCompat) => Ok(()),
            (Executor::OpenaiCompat { .. }, TransportMode::Legacy) => {
                err("openai-compat has no cli binary")
            }
            #[cfg(feature = "acp")]
            (Executor::OpenaiCompat { .. }, TransportMode::Acp) => {
                err("openai-compat does not support acp transport")
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
            // Claude-code in CLI mode goes through the dedicated
            // ClaudeCodeCliTransport (Phase 3 PoC of #1103/#760), which
            // advertises CLI-specific capabilities (resume + native fork +
            // best-effort streaming via `--output-format stream-json`).
            // Other tools' Legacy mode keeps using LegacyTransport — Phase 4
            // will narrow per-tool as more dedicated CLI transports land.
            TransportMode::Legacy => match executor {
                Executor::ClaudeCode { .. } => {
                    Ok(Box::new(ClaudeCodeCliTransport::new(executor.clone())))
                }
                _ => Ok(Box::new(LegacyTransport::new(executor.clone()))),
            },
            TransportMode::Acp => {
                #[cfg(not(feature = "acp"))]
                {
                    let _ = executor;
                    let _ = session_config;
                    anyhow::bail!(
                        "ACP transport is disabled (cargo feature `acp` is not enabled). \
                         Rebuild csa-executor with `--features acp` to enable ACP transport."
                    );
                }
                #[cfg(feature = "acp")]
                {
                    // claude-code ACP transport is gated behind the `claude-code-acp` cargo
                    // feature (disabled by default, #1115/#1117). All other tools' ACP
                    // paths are always available.
                    #[cfg(not(feature = "claude-code-acp"))]
                    if matches!(executor, Executor::ClaudeCode { .. }) {
                        anyhow::bail!(
                            "claude-code ACP transport is disabled (cargo feature `claude-code-acp` \
                         is not enabled). This path was gated because ACP silently crashes at \
                         session startup (turn_count=0, output_log=0B, #1115/#1117). \
                         Rebuild csa-executor with `--features claude-code-acp` to enable \
                         this transport for investigation."
                        );
                    }
                    // codex ACP transport is gated behind the `codex-acp` cargo feature
                    // (disabled by default, #760 / #1128) because native CLI resume
                    // (`codex exec resume <id>`, codex-cli 0.125.0+) is empirically
                    // equivalent on server-side cache reuse.
                    #[cfg(not(feature = "codex-acp"))]
                    if matches!(executor, Executor::Codex { .. }) {
                        anyhow::bail!(
                            "codex ACP transport is disabled (cargo feature `codex-acp` is not \
                         enabled). This path was gated because native `codex exec resume <id>` \
                         (codex-cli 0.125.0+) is empirically equivalent to ACP loadSession on \
                         server-side cache reuse (54%, 44,800/83,503 input tokens, #760 / #1128). \
                         Rebuild csa-executor with `--features codex-acp` to enable this \
                         transport for investigation."
                        );
                    }
                    Ok(Box::new(AcpTransport::new(
                        executor.tool_name(),
                        session_config,
                    )))
                }
            }
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

    /// Test-only: expose the private `mode_for_executor` to sibling test modules.
    #[cfg(test)]
    pub fn mode_for_executor_pub(executor: &Executor) -> Result<TransportMode> {
        Self::mode_for_executor(executor)
    }

    /// Test-only: expose the private `validate_mode_for_executor` to sibling test modules.
    #[cfg(test)]
    pub fn validate_mode_for_executor_pub(
        executor: &Executor,
        mode: TransportMode,
    ) -> Result<(), TransportFactoryError> {
        Self::validate_mode_for_executor(executor, mode)
    }
}

#[cfg(test)]
#[path = "transport_factory_tests.rs"]
mod tests;
