#[async_trait]
impl Transport for AcpTransport {
    fn mode(&self) -> TransportMode {
        TransportMode::Acp
    }

    #[tracing::instrument(skip_all, fields(tool = %self.tool_name))]
    async fn execute(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        options: TransportOptions<'_>,
    ) -> Result<TransportResult> {
        let is_gemini = self.tool_name == "gemini-cli";

        // Non-gemini tools: ACP crash retry count is configured at execution time.
        if !is_gemini {
            return execute_with_crash_retry(
                self, prompt, tool_state, session, extra_env, &options,
            )
            .await;
        }

        // Gemini-cli: 3-phase fallback: OAuth(original) → APIKey(original) → APIKey(flash)
        let max_attempts = gemini_max_attempts(extra_env);
        let has_fallback_key = extra_env
            .is_some_and(|env| env.contains_key(csa_core::gemini::API_KEY_FALLBACK_ENV_KEY));
        let auth_mode = gemini_auth_mode(extra_env).unwrap_or("unknown");
        tracing::debug!(
            max_attempts,
            has_fallback_key,
            auth_mode,
            "gemini-cli ACP retry chain initialized"
        );

        let mut attempt = 1u8;
        let mut retry_phases = Vec::new();
        loop {
            retry_phases.push(GeminiRetryPhase::for_attempt(attempt));
            // Build ACP args for this attempt, injecting model override in phase 3.
            let mut args = self.acp_args.clone();
            if let Some(model) = gemini_retry_model(attempt) {
                tracing::info!(attempt, model, "gemini-cli ACP: overriding model for retry");
                args.extend(["-m".into(), model.into()]);
            }

            // Phase 2+: inject API key auth if available, otherwise keep original env.
            let api_key_env = if gemini_should_use_api_key(attempt) {
                let injected = gemini_inject_api_key_fallback(extra_env);
                if injected.is_none() {
                    tracing::warn!(
                        attempt,
                        auth_mode,
                        has_fallback_key,
                        "gemini-cli ACP: API key fallback unavailable for retry \
                         (auth_mode must be 'oauth' and _CSA_API_KEY_FALLBACK must be set); \
                         retrying with original auth"
                    );
                }
                injected
            } else {
                None
            };
            let attempt_env: Option<&HashMap<String, String>> =
                api_key_env.as_ref().map_or(extra_env, Some);

            // Only resume a provider session on the first attempt; retries start fresh.
            let resume_session_id = if attempt == 1 {
                tool_state.and_then(|s| s.provider_session_id.clone())
            } else {
                None
            };
            if let Some(ref session_id) = resume_session_id {
                tracing::debug!(%session_id, "resuming ACP session from tool state");
            }

            tracing::debug!(
                attempt,
                max_attempts,
                has_api_key_override = api_key_env.is_some(),
                "gemini-cli ACP: executing attempt"
            );

            let result = self
                .execute_acp_attempt(
                    prompt,
                    session,
                    attempt_env,
                    &options,
                    &args,
                    resume_session_id.as_deref(),
                )
                .await;

            let should_retry = match &result {
                Ok(tr) => is_gemini_rate_limited_result(&tr.execution),
                Err(e) => is_gemini_rate_limited_error(&format!("{e:#}")),
            };

            if let Ok(mut transport_result) = result {
                if is_gemini_oauth_prompt_result(&transport_result.execution) {
                    if attempt == 1
                        && gemini_inject_api_key_fallback(extra_env).is_some()
                        && !crate::transport_gemini_retry::gemini_is_no_failover(extra_env)
                    {
                        tracing::warn!(
                            attempt,
                            "gemini-cli ACP OAuth browser prompt detected; retrying with API key"
                        );
                        attempt = attempt.saturating_add(1);
                        continue;
                    }

                    classify_gemini_oauth_prompt_result(&mut transport_result.execution);
                    append_gemini_retry_report(&mut transport_result.execution, &retry_phases);
                    return Ok(transport_result);
                }

                if should_retry && attempt < max_attempts {
                    tracing::info!(
                        attempt,
                        phase_desc = gemini_phase_desc(attempt),
                        "gemini-cli ACP rate limited; advancing phase"
                    );
                    tokio::time::sleep(gemini_rate_limit_backoff(attempt)).await;
                    attempt = attempt.saturating_add(1);
                    continue;
                }

                if should_retry {
                    tracing::warn!(
                        attempt,
                        max_attempts,
                        "gemini-cli ACP: all retry phases exhausted, returning last result"
                    );
                }

                append_gemini_retry_report(&mut transport_result.execution, &retry_phases);
                return Ok(transport_result);
            }

            if should_retry && attempt < max_attempts {
                tracing::info!(
                    attempt,
                    phase_desc = gemini_phase_desc(attempt),
                    "gemini-cli ACP rate limited; advancing phase"
                );
                tokio::time::sleep(gemini_rate_limit_backoff(attempt)).await;
                attempt = attempt.saturating_add(1);
                continue;
            }

            if should_retry {
                tracing::warn!(
                    attempt,
                    max_attempts,
                    "gemini-cli ACP: all retry phases exhausted, returning last result"
                );
            }

            return Err(annotate_gemini_retry_error(
                result.expect_err("only Err remains after Ok path handled"),
                &retry_phases,
            ));
        }
    }

    async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        stream_mode: StreamMode,
        idle_timeout_seconds: u64,
        initial_response_timeout_seconds: Option<u64>,
    ) -> Result<TransportResult> {
        let session = build_ephemeral_meta_session(work_dir);
        self.execute(
            prompt,
            None,
            &session,
            extra_env,
            TransportOptions {
                stream_mode,
                idle_timeout_seconds,
                acp_crash_max_attempts: 2,
                initial_response_timeout_seconds,
                liveness_dead_seconds: csa_process::DEFAULT_LIVENESS_DEAD_SECS,
                stdin_write_timeout_seconds: csa_process::DEFAULT_STDIN_WRITE_TIMEOUT_SECS,
                acp_init_timeout_seconds: 120,
                termination_grace_period_seconds:
                    csa_process::DEFAULT_TERMINATION_GRACE_PERIOD_SECS,
                output_spool: None,
                output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
                output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
                setting_sources: None,
                sandbox: None,
            },
        )
        .await
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
