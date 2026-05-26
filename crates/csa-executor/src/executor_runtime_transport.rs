impl Executor {
    /// Override codex runtime transport metadata.
    pub fn override_codex_transport(&mut self, transport: CodexTransport) {
        if let Self::Codex {
            runtime_metadata, ..
        } = self
        {
            *runtime_metadata = runtime_metadata.with_transport(transport);
        }
    }

    #[must_use]
    pub fn codex_transport(&self) -> Option<CodexTransport> {
        match self {
            Self::Codex {
                runtime_metadata, ..
            } => Some(runtime_metadata.transport_mode()),
            _ => None,
        }
    }

    pub fn enable_codex_fast_mode(&mut self) {
        if let Self::Codex {
            runtime_metadata, ..
        } = self
        {
            *runtime_metadata = runtime_metadata.with_fast_mode(true);
        }
    }

    pub fn set_codex_tmux_mode(&mut self, enabled: bool) {
        if let Self::Codex {
            runtime_metadata, ..
        } = self
        {
            *runtime_metadata = runtime_metadata.with_tmux_mode(enabled);
        }
    }

    #[must_use]
    pub fn codex_fast_mode_enabled(&self) -> bool {
        match self {
            Self::Codex {
                runtime_metadata, ..
            } => runtime_metadata.fast_mode_enabled(),
            _ => false,
        }
    }

    #[must_use]
    pub fn codex_tmux_mode_enabled(&self) -> bool {
        match self {
            Self::Codex {
                runtime_metadata, ..
            } => runtime_metadata.tmux_mode_enabled(),
            _ => false,
        }
    }

    /// Override claude-code runtime transport metadata.
    pub fn override_claude_code_transport(&mut self, transport: ClaudeCodeTransport) {
        if let Self::ClaudeCode {
            runtime_metadata, ..
        } = self
        {
            *runtime_metadata = ClaudeCodeRuntimeMetadata::from_transport(transport);
        }
    }

    #[must_use]
    pub fn claude_code_transport(&self) -> Option<ClaudeCodeTransport> {
        match self {
            Self::ClaudeCode {
                runtime_metadata, ..
            } => Some(runtime_metadata.transport_mode()),
            _ => None,
        }
    }
}
