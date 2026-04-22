impl Executor {
    /// Override codex runtime transport metadata.
    pub fn override_codex_transport(&mut self, transport: CodexTransport) {
        if let Self::Codex {
            runtime_metadata, ..
        } = self
        {
            *runtime_metadata = CodexRuntimeMetadata::from_transport(transport);
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
