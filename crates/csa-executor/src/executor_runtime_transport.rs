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

    /// Execute in a specific directory (ephemeral sessions, `extra_env` for API keys etc.).
    ///
    /// `subtree_pin` carries CSA's authoritative subtree model pin (#1741),
    /// out-of-band from `extra_env`; it is the only channel that may set the
    /// pin keys on the child. Pass `None` when CSA did not decide to pin.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
        allow_git_push: bool,
        stream_mode: csa_process::StreamMode,
        idle_timeout_seconds: u64,
        initial_response_timeout: ResolvedTimeout,
    ) -> Result<ExecutionResult> {
        Ok(self
            .execute_in_with_transport(
                prompt,
                work_dir,
                extra_env,
                subtree_pin,
                allow_git_push,
                stream_mode,
                idle_timeout_seconds,
                initial_response_timeout,
            )
            .await?
            .execution)
    }

    /// Execute in a specific directory and keep transport metadata.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_in_with_transport(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
        allow_git_push: bool,
        stream_mode: csa_process::StreamMode,
        idle_timeout_seconds: u64,
        initial_response_timeout: ResolvedTimeout,
    ) -> Result<TransportResult> {
        let transport = self.transport(None)?;
        let mut result = transport
            .execute_in(
                prompt,
                work_dir,
                extra_env,
                subtree_pin,
                allow_git_push,
                stream_mode,
                idle_timeout_seconds,
                initial_response_timeout,
            )
            .await?;
        result.execution.consolidate_stderr_retries();
        Ok(result)
    }
}
