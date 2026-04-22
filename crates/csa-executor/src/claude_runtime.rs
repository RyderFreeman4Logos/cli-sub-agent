//! Build-dependent runtime metadata for the claude-code tool.
//!
//! Claude Code defaults to ACP for now, but callers can opt into the native
//! CLI transport (`claude -p/--print`) via config. Keeping transport metadata
//! here lets availability checks and executor dispatch agree on the runtime
//! binary without hardcoded string forks.

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
    /// Current default transport for this build.
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

    #[test]
    fn current_build_defaults_to_acp() {
        let meta = claude_runtime_metadata();

        assert_eq!(meta.transport_mode(), ClaudeCodeTransport::Acp);
        assert_eq!(meta.runtime_binary_name(), "claude-code-acp");
    }
}
