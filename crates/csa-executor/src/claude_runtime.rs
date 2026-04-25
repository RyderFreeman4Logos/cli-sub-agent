//! Build-dependent runtime metadata for the claude-code tool.
//!
//! Claude Code now defaults to the CLI transport (`ClaudeCodeCliTransport`,
//! `TransportMode::Legacy`) as a workaround for ACP startup-crash bugs
//! (#1115/#1117). The ACP path is still compiled in but gated behind the
//! `claude-code-acp` cargo feature (default OFF) on `csa-executor`. Callers
//! can request ACP explicitly via config (`transport = "acp"`) with the
//! feature enabled; without the feature, an explicit ACP request is rejected
//! with a clear error pointing to the rebuild flag.
//!
//! Keeping transport metadata here lets availability checks and executor
//! dispatch agree on the runtime binary without hardcoded string forks.

use serde::{Deserialize, Serialize};

use crate::install_hints::{CLAUDE_CODE_ACP_INSTALL_HINT, CLAUDE_CODE_CLI_INSTALL_HINT};

/// Claude Code transport mode selected for the current runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaudeCodeTransport {
    Cli,
    Acp,
}

impl ClaudeCodeTransport {
    /// Default transport for serialization and fallback deserialization.
    ///
    /// NOTE: `default_for_build` returns `Acp` at the metadata level, but
    /// `TransportFactory::mode_for_executor` routes `claude-code` to
    /// `TransportMode::Legacy` (CLI) by default as a workaround for ACP
    /// startup-crash bugs (#1115/#1117). ACP is only reachable for claude-code
    /// when the `claude-code-acp` cargo feature is enabled AND the executor
    /// carries explicit `Some(Acp)` transport metadata.
    #[must_use]
    pub const fn default_for_build() -> Self {
        Self::Acp
    }

    #[must_use]
    pub const fn runtime_binary_name(self) -> &'static str {
        match self {
            Self::Cli => "claude",
            Self::Acp => "claude-code-acp",
        }
    }

    #[must_use]
    pub const fn install_hint(self) -> &'static str {
        match self {
            Self::Cli => CLAUDE_CODE_CLI_INSTALL_HINT,
            Self::Acp => CLAUDE_CODE_ACP_INSTALL_HINT,
        }
    }
}

impl Default for ClaudeCodeTransport {
    fn default() -> Self {
        Self::default_for_build()
    }
}

/// Unified view of claude-code runtime metadata for the current build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeCodeRuntimeMetadata {
    #[serde(default = "ClaudeCodeTransport::default_for_build")]
    transport: ClaudeCodeTransport,
}

impl ClaudeCodeRuntimeMetadata {
    #[must_use]
    pub const fn current() -> Self {
        Self::from_transport(ClaudeCodeTransport::default_for_build())
    }

    #[must_use]
    pub const fn from_transport(transport: ClaudeCodeTransport) -> Self {
        Self { transport }
    }

    #[must_use]
    pub const fn transport_mode(self) -> ClaudeCodeTransport {
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
}

impl Default for ClaudeCodeRuntimeMetadata {
    fn default() -> Self {
        Self::current()
    }
}

#[must_use]
pub const fn claude_runtime_metadata() -> ClaudeCodeRuntimeMetadata {
    ClaudeCodeRuntimeMetadata::current()
}

#[cfg(test)]
mod tests {
    use super::{ClaudeCodeRuntimeMetadata, ClaudeCodeTransport, claude_runtime_metadata};

    #[test]
    fn explicit_cli_metadata_is_correct() {
        let meta = ClaudeCodeRuntimeMetadata::from_transport(ClaudeCodeTransport::Cli);

        assert_eq!(meta.transport_mode(), ClaudeCodeTransport::Cli);
        assert_eq!(meta.runtime_binary_name(), "claude");
    }

    #[test]
    fn explicit_acp_metadata_is_correct() {
        let meta = ClaudeCodeRuntimeMetadata::from_transport(ClaudeCodeTransport::Acp);

        assert_eq!(meta.transport_mode(), ClaudeCodeTransport::Acp);
        assert_eq!(meta.runtime_binary_name(), "claude-code-acp");
    }

    // NOTE: The metadata-level default is still `Acp` for serde deserialization
    // compatibility. The actual runtime routing to CLI happens in
    // `TransportFactory::mode_for_executor` (#1115/#1117 workaround).
    #[test]
    fn metadata_default_for_build_is_acp() {
        let meta = claude_runtime_metadata();

        assert_eq!(meta.transport_mode(), ClaudeCodeTransport::Acp);
        assert_eq!(meta.runtime_binary_name(), "claude-code-acp");
    }
}
