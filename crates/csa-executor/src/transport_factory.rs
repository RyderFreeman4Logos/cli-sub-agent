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
            "claude-code" | "codex" | "gemini-cli" => TransportMode::Acp,
            "openai-compat" => TransportMode::OpenaiCompat,
            _ => TransportMode::Legacy,
        }
    }

    pub fn create(
        executor: &Executor,
        session_config: Option<SessionConfig>,
    ) -> Box<dyn Transport> {
        match Self::mode_for_tool(executor.tool_name()) {
            TransportMode::Legacy => Box::new(LegacyTransport::new(executor.clone())),
            TransportMode::Acp => Box::new(AcpTransport::new(executor.tool_name(), session_config)),
            TransportMode::OpenaiCompat => {
                let default_model = if let Executor::OpenaiCompat { model_override, .. } = executor
                {
                    model_override.clone()
                } else {
                    None
                };
                Box::new(crate::transport_openai_compat::OpenaiCompatTransport::new(
                    default_model,
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
