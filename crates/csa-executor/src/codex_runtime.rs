//! Codex now defaults to the CLI transport (`LegacyTransport`,
//! `TransportMode::Legacy`) using `codex exec resume <session_id>` (codex-cli
//! 0.125.0+). The native CLI resume is empirically equivalent to ACP
//! `loadSession` on server-side cache reuse (54%, 44,800/83,503 input tokens,
//! #760 / #1128). The ACP path is still compiled in but gated behind the
//! `codex-acp` cargo feature (default OFF) on `csa-executor`. Callers can
//! request ACP explicitly via config (`transport = "acp"`) with the feature
//! enabled; without the feature, an explicit ACP request is rejected with a
//! clear error pointing to the rebuild flag.
//!
//! Keeping transport metadata here lets availability checks and executor
//! dispatch agree on the runtime binary without hardcoded string forks.

use serde::{Deserialize, Serialize};

/// Codex transport mode selected for the current runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodexTransport {
    Cli,
    Acp,
}

impl CodexTransport {
    /// Default transport for serialization and fallback deserialization.
    ///
    /// NOTE: `default_for_build` returns `Acp` at the metadata level, but
    /// `TransportFactory::mode_for_executor` routes `codex` to
    /// `TransportMode::Legacy` (CLI) by default in favour of native
    /// `codex exec resume <id>` (#760 / #1128). ACP is only reachable for
    /// codex when the `codex-acp` cargo feature is enabled AND the executor
    /// carries explicit `Some(Acp)` transport metadata.
    #[must_use]
    pub const fn default_for_build() -> Self {
        Self::Acp
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

impl Default for CodexTransport {
    fn default() -> Self {
        Self::default_for_build()
    }
}

/// Unified view of codex runtime metadata for the current build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexRuntimeMetadata {
    #[serde(default = "CodexTransport::default_for_build")]
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

impl Default for CodexRuntimeMetadata {
    fn default() -> Self {
        Self::current()
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

    // NOTE: The metadata-level default is still `Acp` for serde deserialization
    // compatibility. The actual runtime routing to CLI happens in
    // `TransportFactory::mode_for_executor` (#760 / #1128 transport flip).
    #[test]
    fn metadata_default_for_build_is_acp() {
        let meta = codex_runtime_metadata();

        assert_eq!(meta.transport_mode(), CodexTransport::Acp);
        assert_eq!(meta.runtime_binary_name(), "codex-acp");
    }
}
