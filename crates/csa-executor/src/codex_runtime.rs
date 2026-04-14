//! Build-dependent runtime metadata for the codex tool.
//!
//! T1 centralizes codex runtime metadata here without flipping the default
//! transport yet. Later tasks can rewire downstream callers to consult this
//! module instead of duplicating hardcoded `"codex-acp"` assumptions.

/// Codex transport mode selected for the current runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexTransport {
    Cli,
    Acp,
}

impl CodexTransport {
    /// Current default transport for this build.
    ///
    /// T1 preserves the existing ACP default while the `codex-acp` feature is
    /// enabled. T3 will flip this default without changing the metadata API.
    #[must_use]
    pub const fn default_for_build() -> Self {
        if CodexRuntimeMetadata::acp_compiled_in() {
            Self::Acp
        } else {
            Self::Cli
        }
    }

    #[must_use]
    pub const fn runtime_binary_name(self) -> &'static str {
        match self {
            Self::Cli => "codex",
            Self::Acp => "codex-acp",
        }
    }

    #[must_use]
    pub const fn install_hint(self) -> &'static str {
        match self {
            Self::Cli => "Install: npm install -g @openai/codex",
            Self::Acp => "Install ACP adapter: npm install -g @zed-industries/codex-acp",
        }
    }
}

/// Unified view of codex runtime metadata for the current build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexRuntimeMetadata {
    transport: CodexTransport,
}

impl CodexRuntimeMetadata {
    #[must_use]
    pub const fn current() -> Self {
        Self::from_transport(CodexTransport::default_for_build())
    }

    #[must_use]
    pub const fn from_transport(transport: CodexTransport) -> Self {
        Self { transport }
    }

    #[must_use]
    pub const fn transport_mode(self) -> CodexTransport {
        self.transport
    }

    #[must_use]
    pub const fn runtime_binary_name(self) -> &'static str {
        self.transport.runtime_binary_name()
    }

    #[must_use]
    pub const fn install_hint(self) -> &'static str {
        self.transport.install_hint()
    }

    #[must_use]
    pub const fn acp_compiled_in() -> bool {
        cfg!(feature = "codex-acp")
    }
}

#[must_use]
pub const fn codex_runtime_metadata() -> CodexRuntimeMetadata {
    CodexRuntimeMetadata::current()
}

#[cfg(test)]
mod tests {
    use super::{CodexRuntimeMetadata, CodexTransport, codex_runtime_metadata};

    #[test]
    fn explicit_cli_metadata_is_correct() {
        let meta = CodexRuntimeMetadata::from_transport(CodexTransport::Cli);

        assert_eq!(meta.transport_mode(), CodexTransport::Cli);
        assert_eq!(meta.runtime_binary_name(), "codex");
        assert_eq!(meta.install_hint(), "Install: npm install -g @openai/codex");
    }

    #[test]
    fn explicit_acp_metadata_is_correct() {
        let meta = CodexRuntimeMetadata::from_transport(CodexTransport::Acp);

        assert_eq!(meta.transport_mode(), CodexTransport::Acp);
        assert_eq!(meta.runtime_binary_name(), "codex-acp");
        assert_eq!(
            meta.install_hint(),
            "Install ACP adapter: npm install -g @zed-industries/codex-acp"
        );
    }

    #[cfg(feature = "codex-acp")]
    #[test]
    fn current_build_defaults_to_acp_when_feature_enabled() {
        let meta = codex_runtime_metadata();

        assert!(CodexRuntimeMetadata::acp_compiled_in());
        assert_eq!(meta.transport_mode(), CodexTransport::Acp);
        assert_eq!(meta.runtime_binary_name(), "codex-acp");
    }

    #[cfg(not(feature = "codex-acp"))]
    #[test]
    fn current_build_defaults_to_cli_when_feature_disabled() {
        let meta = codex_runtime_metadata();

        assert!(!CodexRuntimeMetadata::acp_compiled_in());
        assert_eq!(meta.transport_mode(), CodexTransport::Cli);
        assert_eq!(meta.runtime_binary_name(), "codex");
    }
}
