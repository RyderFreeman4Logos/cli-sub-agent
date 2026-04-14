use anyhow::Result;

use crate::executor::Executor;
use csa_acp::SessionConfig;

use super::{AcpTransport, LegacyTransport, Transport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    Legacy,
    Acp,
    OpenaiCompat,
}

pub struct TransportFactory;

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
                    #[cfg(feature = "codex-acp")]
                    {
                        Ok(TransportMode::Acp)
                    }
                    #[cfg(not(feature = "codex-acp"))]
                    {
                        Err(anyhow::anyhow!(
                            "codex transport 'acp' requires the `codex-acp` cargo feature; rebuild with `cargo build --features codex-acp`"
                        ))
                    }
                }
            },
            _ => Ok(Self::mode_for_tool(executor.tool_name())),
        }
    }

    pub fn create(
        executor: &Executor,
        session_config: Option<SessionConfig>,
    ) -> Result<Box<dyn Transport>> {
        match Self::mode_for_executor(executor)? {
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
