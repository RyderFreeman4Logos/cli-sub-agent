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
            if is_gemini_oauth_prompt_result(&result.execution) {
                if attempt == 1
                    && gemini_inject_api_key_fallback(extra_env).is_some()
                    && !crate::transport_gemini_retry::gemini_is_no_failover(extra_env)
                {
                    tracing::warn!(
                        attempt,
                        "gemini-cli legacy OAuth browser prompt detected; retrying with API key"
                    );
                    attempt = attempt.saturating_add(1);
                    continue;
                }

                let mut result = result;
                classify_gemini_oauth_prompt_result(&mut result.execution);
                return Ok(result);
            }
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
            let codex_timeout = Self::consume_resolved_transport_initial_response_timeout_seconds(
                options.initial_response_timeout,
            );
            let retry_executor = executor.clone();
            let retry_options = options.clone();
            let result = apply_and_maybe_retry_codex_exec_initial_stall(
                    &executor,
                    result,
                    codex_timeout,
                    |retry_budget| async move {
                        let mut downgraded_executor = retry_executor;
                        downgraded_executor.override_thinking_budget(retry_budget);
                        let retry_result = self
                            .execute_single_attempt(
                                &downgraded_executor,
                                prompt,
                                tool_state,
                                session,
                                attempt_env,
                                retry_options,
                            )
                            .await?;
                        Ok((downgraded_executor, retry_result))
                    },
                )
                .await?;
            if let Some(classification) = classify_gemini_legacy_initial_stall(
                &executor,
                &result.execution,
                codex_timeout,
            ) {
                let mut result = result;
                apply_gemini_legacy_initial_stall_summary(&mut result.execution, &classification);
                return Ok(result);
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
        initial_response_timeout: super::ResolvedTimeout,
    ) -> Result<TransportResult> {
        LegacyTransport::execute_in(
            self,
            prompt,
            work_dir,
            extra_env,
            stream_mode,
            idle_timeout_seconds,
            initial_response_timeout,
        )
        .await
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
