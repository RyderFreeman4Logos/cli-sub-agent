#[async_trait]
impl Transport for LegacyTransport {
    fn mode(&self) -> TransportMode {
        TransportMode::Legacy
    }

    fn capabilities(&self) -> super::TransportCapabilities {
        super::TransportCapabilities {
            streaming: false,
            session_resume: !matches!(
                self.executor.claude_code_transport(),
                Some(crate::ClaudeCodeTransport::Cli)
            ),
            session_fork: false,
            typed_events: false,
        }
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
            let mut retried_degraded_mcp = false;

            // Phase 2+: inject API key auth if available, otherwise keep original env.
            let api_key_env = if gemini_should_use_api_key(attempt) {
                gemini_inject_api_key_fallback(extra_env)
            } else {
                None
            };
            let mut prepared_attempt_env = api_key_env
                .as_ref()
                .map_or_else(|| extra_env.cloned().unwrap_or_default(), Clone::clone);
            let (gemini_shared_npm_cache_raw_path, gemini_shared_npm_cache_source) =
                if executor.tool_name() == "gemini-cli" {
                    let source_home = prepared_attempt_env
                        .get("HOME")
                        .cloned()
                        .or_else(|| std::env::var("HOME").ok())
                        .map(PathBuf::from);
                    (
                        shared_npm_cache_dir(&prepared_attempt_env, source_home.as_deref()),
                        resolve_gemini_shared_npm_cache_source(
                            &prepared_attempt_env,
                            source_home.as_deref(),
                        ),
                    )
                } else {
                    (None, None)
                };
            let mut gemini_runtime_home = None;
            let mut mcp_diagnostic = None;
            let allow_degraded_mcp = if executor.tool_name() == "gemini-cli" {
                let session_dir = csa_session::manager::get_session_dir(
                    Path::new(&session.project_path),
                    &session.meta_session_id,
                )
                .ok();
                let runtime_home = prepare_gemini_runtime_env(
                    &mut prepared_attempt_env,
                    Some(Path::new(&session.project_path)),
                    session_dir.as_deref(),
                    &session.meta_session_id,
                )?;
                let diagnostic = diagnose_mcp_init_failure(
                    &runtime_home,
                    prepared_attempt_env.get("PATH").map(std::ffi::OsStr::new),
                );
                gemini_runtime_home = Some(runtime_home);
                mcp_diagnostic = Some(diagnostic);
                gemini_allow_degraded_mcp(&prepared_attempt_env)
            } else {
                true
            };

            let result = loop {
                let result = self
                    .execute_single_attempt(
                        &executor,
                        prompt,
                        tool_state,
                        session,
                        LegacyAttemptEnv {
                            extra_env: Some(&prepared_attempt_env),
                            gemini_shared_npm_cache_raw_path: gemini_shared_npm_cache_raw_path
                                .as_deref(),
                            gemini_shared_npm_cache_source,
                        },
                        options.clone(),
                    )
                    .await?;
                if executor.tool_name() != "gemini-cli"
                    || !is_gemini_mcp_issue_result(&result.execution)
                    || retried_degraded_mcp
                    || !allow_degraded_mcp
                {
                    break result;
                }

                let diagnostic = mcp_diagnostic
                    .clone()
                    .unwrap_or_else(|| {
                        diagnose_mcp_init_failure(
                            gemini_runtime_home.as_deref().expect("gemini runtime home"),
                            prepared_attempt_env.get("PATH").map(std::ffi::OsStr::new),
                        )
                    });
                let disable_all = diagnostic.unhealthy_servers.is_empty();
                if let Some(runtime_home) = gemini_runtime_home.as_deref() {
                    disable_mcp_servers_in_runtime(runtime_home, &diagnostic, disable_all)?;
                }
                tracing::warn!(
                    unhealthy_servers = %diagnostic.unhealthy_servers.join(","),
                    disable_all,
                    "gemini-cli reported MCP startup issues; retrying with degraded MCP"
                );
                mcp_diagnostic = Some(diagnostic);
                retried_degraded_mcp = true;
            };
            let mut result = result;
            if retried_degraded_mcp {
                let warning_summary = format_mcp_init_warning_summary(
                    mcp_diagnostic
                        .as_ref()
                        .expect("degraded MCP retry should preserve diagnostic"),
                    mcp_diagnostic
                        .as_ref()
                        .is_some_and(|diagnostic| diagnostic.unhealthy_servers.is_empty()),
                );
                apply_gemini_mcp_warning_summary(&mut result.execution, &warning_summary);
            } else if executor.tool_name() == "gemini-cli"
                && is_gemini_mcp_issue_result(&result.execution)
                && !allow_degraded_mcp
            {
                let warning_summary = format_mcp_init_warning_summary(
                    mcp_diagnostic
                        .as_ref()
                        .expect("gemini diagnostic should exist"),
                    mcp_diagnostic
                        .as_ref()
                        .is_some_and(|diagnostic| diagnostic.unhealthy_servers.is_empty()),
                );
                apply_gemini_mcp_warning_summary(&mut result.execution, &warning_summary);
            }
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
                                LegacyAttemptEnv {
                                    extra_env: Some(&prepared_attempt_env),
                                    gemini_shared_npm_cache_raw_path: gemini_shared_npm_cache_raw_path
                                        .as_deref(),
                                    gemini_shared_npm_cache_source,
                                },
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

impl LegacyTransport {
    /// Execute the legacy direct-entry path.
    ///
    /// `initial_response_timeout_seconds` is already resolved by
    /// `Executor::execute_in_with_transport()`: `None` means disabled, and positive values are
    /// concrete watchdog durations. This layer must not re-apply executor defaults.
    pub async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        stream_mode: StreamMode,
        idle_timeout_seconds: u64,
        initial_response_timeout: ResolvedTimeout,
    ) -> Result<TransportResult> {
        let has_fallback_key = extra_env
            .is_some_and(|env| env.contains_key(csa_core::gemini::API_KEY_FALLBACK_ENV_KEY));
        let auth_mode = gemini_auth_mode(extra_env).unwrap_or("unknown");
        let max_attempts = gemini_max_attempts(extra_env);
        tracing::debug!(
            max_attempts,
            has_fallback_key,
            auth_mode,
            "gemini-cli legacy retry chain initialized"
        );

        let mut attempt = 1u8;
        loop {
            let executor = self.executor_for_attempt(attempt);
            let mut retried_degraded_mcp = false;

            let api_key_env = if gemini_should_use_api_key(attempt) {
                let injected = gemini_inject_api_key_fallback(extra_env);
                if injected.is_none() {
                    tracing::warn!(
                        attempt,
                        auth_mode,
                        has_fallback_key,
                        "gemini-cli legacy: API key fallback unavailable for retry \
                         (auth_mode must be 'oauth' and _CSA_API_KEY_FALLBACK must be set); \
                         retrying with original auth"
                    );
                }
                injected
            } else {
                None
            };
            let mut prepared_attempt_env = api_key_env
                .as_ref()
                .map_or_else(|| extra_env.cloned().unwrap_or_default(), Clone::clone);
            let mut gemini_runtime_home = None;
            let mut mcp_diagnostic = None;
            let allow_degraded_mcp = if executor.tool_name() == "gemini-cli" {
                let session_id = format!("execute-in-{}", new_session_id());
                let runtime_home = prepare_gemini_runtime_env(
                    &mut prepared_attempt_env,
                    Some(work_dir),
                    None,
                    &session_id,
                )?;
                let diagnostic = diagnose_mcp_init_failure(
                    &runtime_home,
                    prepared_attempt_env.get("PATH").map(std::ffi::OsStr::new),
                );
                gemini_runtime_home = Some(runtime_home);
                mcp_diagnostic = Some(diagnostic);
                gemini_allow_degraded_mcp(&prepared_attempt_env)
            } else {
                true
            };

            let result = loop {
                let result = self
                    .execute_in_single_attempt(ExecuteInAttempt {
                        executor: &executor,
                        prompt,
                        work_dir,
                        extra_env: Some(&prepared_attempt_env),
                        stream_mode,
                        idle_timeout_seconds,
                        resolved_initial_response_timeout: initial_response_timeout,
                    })
                    .await?;
                if executor.tool_name() != "gemini-cli"
                    || !is_gemini_mcp_issue_result(&result.execution)
                    || retried_degraded_mcp
                    || !allow_degraded_mcp
                {
                    break result;
                }

                let diagnostic = mcp_diagnostic.clone().unwrap_or_else(|| {
                    diagnose_mcp_init_failure(
                        gemini_runtime_home
                            .as_deref()
                            .expect("gemini runtime home"),
                        prepared_attempt_env.get("PATH").map(std::ffi::OsStr::new),
                    )
                });
                let disable_all = diagnostic.unhealthy_servers.is_empty();
                if let Some(runtime_home) = gemini_runtime_home.as_deref() {
                    disable_mcp_servers_in_runtime(runtime_home, &diagnostic, disable_all)?;
                }
                tracing::warn!(
                    unhealthy_servers = %diagnostic.unhealthy_servers.join(","),
                    disable_all,
                    "gemini-cli execute_in reported MCP startup issues; retrying with degraded MCP"
                );
                mcp_diagnostic = Some(diagnostic);
                retried_degraded_mcp = true;
            };
            let mut result = result;
            if retried_degraded_mcp {
                let warning_summary = format_mcp_init_warning_summary(
                    mcp_diagnostic
                        .as_ref()
                        .expect("degraded MCP retry should preserve diagnostic"),
                    mcp_diagnostic
                        .as_ref()
                        .is_some_and(|diagnostic| diagnostic.unhealthy_servers.is_empty()),
                );
                apply_gemini_mcp_warning_summary(&mut result.execution, &warning_summary);
            } else if executor.tool_name() == "gemini-cli"
                && is_gemini_mcp_issue_result(&result.execution)
                && !allow_degraded_mcp
            {
                let warning_summary = format_mcp_init_warning_summary(
                    mcp_diagnostic
                        .as_ref()
                        .expect("gemini diagnostic should exist"),
                    mcp_diagnostic
                        .as_ref()
                        .is_some_and(|diagnostic| diagnostic.unhealthy_servers.is_empty()),
                );
                apply_gemini_mcp_warning_summary(&mut result.execution, &warning_summary);
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
            let direct_timeout = consume_resolved_execute_in_initial_response_timeout_seconds(
                initial_response_timeout,
            );
            let retry_executor = executor.clone();
            let result = apply_and_maybe_retry_codex_exec_initial_stall(
                &executor,
                result,
                direct_timeout,
                |retry_budget| async move {
                    let mut downgraded_executor = retry_executor;
                    downgraded_executor.override_thinking_budget(retry_budget);
                    let retry_result = self
                        .execute_in_single_attempt(ExecuteInAttempt {
                            executor: &downgraded_executor,
                            prompt,
                            work_dir,
                            extra_env: Some(&prepared_attempt_env),
                            stream_mode,
                            idle_timeout_seconds,
                            resolved_initial_response_timeout: initial_response_timeout,
                        })
                        .await?;
                    Ok((downgraded_executor, retry_result))
                },
            )
            .await?;
            if let Some(classification) =
                classify_gemini_legacy_initial_stall(&executor, &result.execution, direct_timeout)
            {
                let mut result = result;
                apply_gemini_legacy_initial_stall_summary(&mut result.execution, &classification);
                return Ok(result);
            }
            return Ok(result);
        }
    }
}
