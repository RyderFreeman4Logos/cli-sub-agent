#[async_trait]
impl Transport for LegacyTransport {
    fn mode(&self) -> TransportMode {
        TransportMode::Legacy
    }

    async fn execute(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        options: TransportOptions<'_>,
    ) -> Result<TransportResult> {
        // 3-phase fallback: OAuth(original) → APIKey(original) → APIKey(flash)
        let mut attempt = 1u8;
        loop {
            let executor = self.executor_for_attempt(attempt);

            // Phase 2+: inject API key auth if available, otherwise keep original env.
            let api_key_env = if gemini_should_use_api_key(attempt) {
                gemini_inject_api_key_fallback(extra_env)
            } else {
                None
            };
            let attempt_env = api_key_env.as_ref().map_or(extra_env, Some);

            let result = self
                .execute_single_attempt(
                    &executor,
                    prompt,
                    tool_state,
                    session,
                    attempt_env,
                    options.clone(),
                )
                .await?;
            if let Some(backoff) =
                self.should_retry_gemini_rate_limited(&result.execution, attempt, extra_env)
            {
                let phase_desc = match attempt {
                    1 => "OAuth→APIKey(same model)",
                    2 => "APIKey(same model)→APIKey(flash)",
                    _ => "final",
                };
                tracing::info!(
                    attempt,
                    phase_desc,
                    "gemini-cli rate limited; advancing phase"
                );
                tokio::time::sleep(backoff).await;
                attempt = attempt.saturating_add(1);
                continue;
            }
            return Ok(result);
        }
    }

    async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        stream_mode: StreamMode,
        idle_timeout_seconds: u64,
    ) -> Result<TransportResult> {
        LegacyTransport::execute_in(
            self,
            prompt,
            work_dir,
            extra_env,
            stream_mode,
            idle_timeout_seconds,
        )
        .await
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
