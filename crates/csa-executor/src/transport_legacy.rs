use super::*;

#[derive(Debug, Clone)]
pub struct LegacyTransport {
    pub(super) executor: Executor,
}

struct ExecuteInAttempt<'a> {
    executor: &'a Executor,
    prompt: &'a str,
    work_dir: &'a Path,
    extra_env: Option<&'a HashMap<String, String>>,
    subtree_pin: Option<&'a csa_core::env::SubtreeModelPin>,
    allow_git_push: bool,
    stream_mode: StreamMode,
    idle_timeout_seconds: u64,
    /// Already resolved by `Executor::execute_in_with_transport()`.
    ///
    /// Contract:
    /// - `None` disables the watchdog
    /// - `Some(seconds > 0)` arms the watchdog for that duration
    /// - `Some(0)` should not reach this layer, but is treated as disabled
    resolved_initial_response_timeout: ResolvedTimeout,
}

#[derive(Clone, Copy)]
struct LegacyAttemptEnv<'a> {
    extra_env: Option<&'a HashMap<String, String>>,
    gemini_shared_npm_cache_raw_path: Option<&'a Path>,
    gemini_shared_npm_cache_source: Option<transport_gemini_helpers::GeminiSharedNpmCacheSource>,
    clean_contract: Option<&'a crate::command_isolation::CleanCommandContract>,
}

impl LegacyTransport {
    pub fn new(executor: Executor) -> Self {
        Self { executor }
    }

    pub(super) fn should_retry_gemini_rate_limited(
        &self,
        execution: &ExecutionResult,
        attempt: u8,
        extra_env: Option<&HashMap<String, String>>,
    ) -> Option<Duration> {
        let max = gemini_max_attempts(extra_env);
        if !matches!(self.executor, Executor::GeminiCli { .. })
            || attempt >= max
            || detect_gemini_permanent_quota_exhaustion_result(execution).is_some()
            || !is_gemini_rate_limited_result(execution)
        {
            return None;
        }
        Some(gemini_rate_limit_backoff(attempt))
    }

    pub(super) fn should_retry_codex_rate_limited(
        &self,
        execution: &ExecutionResult,
        retry_count: u8,
        extra_env: Option<&HashMap<String, String>>,
    ) -> Option<Duration> {
        if !matches!(self.executor, Executor::Codex { .. })
            || retry_count >= CODEX_RATE_LIMIT_MAX_RETRIES
            || extra_env.is_some_and(|env| env.contains_key(csa_core::env::NO_FAILOVER_ENV_KEY))
            || is_codex_permanent_quota_result(execution)
            || !is_codex_transient_rate_limit_result(execution)
        {
            return None;
        }
        Some(codex_rate_limit_backoff(retry_count))
    }

    pub(super) fn executor_for_attempt(&self, attempt: u8) -> Executor {
        match &self.executor {
            Executor::GeminiCli {
                thinking_budget, ..
            } => {
                if let Some(model) = gemini_retry_model(attempt) {
                    Executor::GeminiCli {
                        model_override: Some(model.to_string()),
                        thinking_budget: thinking_budget.clone(),
                    }
                } else {
                    self.executor.clone()
                }
            }
            _ => self.executor.clone(),
        }
    }

    async fn execute_in_single_attempt(
        &self,
        request: ExecuteInAttempt<'_>,
    ) -> Result<TransportResult> {
        // Stage the antigravity-cli `model` field in
        // `~/.gemini/antigravity-cli/settings.json` before spawning `agy`,
        // and hold the RAII guard so the original contents are restored
        // when this function returns (#1620). For all other executors this
        // is `None` and a no-op.
        let _antigravity_guard = request.executor.antigravity_settings_guard()?;
        let (cmd, stdin_data) = request
            .executor
            .build_execute_in_command_with_git_push_allowed(
                request.prompt,
                request.work_dir,
                request.extra_env,
                request.subtree_pin,
                request.allow_git_push,
            );
        let spawn_options = SpawnOptions {
            stdin_write_timeout: std::time::Duration::from_secs(
                csa_process::DEFAULT_STDIN_WRITE_TIMEOUT_SECS,
            ),
            keep_stdin_open: false,
            spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
            keep_rotated_spool: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
            // execute_in (ephemeral/testing path) keeps the #1652 scan enabled.
            error_marker_scan_enabled: true,
        };
        let initial_response_timeout_seconds =
            consume_resolved_execute_in_initial_response_timeout_seconds(
                request.resolved_initial_response_timeout,
            );
        let child = spawn_tool_with_options(cmd, stdin_data, spawn_options).await?;
        let child_pid = child.id();
        let execution = wait_and_capture_with_idle_timeout(
            child,
            request.stream_mode,
            std::time::Duration::from_secs(request.idle_timeout_seconds),
            std::time::Duration::from_secs(csa_process::DEFAULT_LIVENESS_DEAD_SECS),
            std::time::Duration::from_secs(csa_process::DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
            None,
            spawn_options,
            initial_response_timeout_seconds.map(std::time::Duration::from_secs),
        )
        .await?;
        if let Some(classification) = classify_codex_exec_initial_stall(
            request.executor,
            &execution,
            initial_response_timeout_seconds,
        ) {
            log_codex_exec_initial_stall(&classification, child_pid);
        }
        Ok(TransportResult {
            execution,
            provider_session_id: None,
            events: Vec::new(),
            metadata: Default::default(),
        })
    }

    pub(super) fn consume_resolved_transport_initial_response_timeout_seconds(
        resolved_timeout: ResolvedTimeout,
    ) -> Option<u64> {
        consume_resolved_initial_response_timeout_seconds(resolved_timeout)
    }

    async fn execute_single_attempt(
        &self,
        executor: &Executor,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        attempt_env: LegacyAttemptEnv<'_>,
        options: TransportOptions<'_>,
    ) -> Result<TransportResult> {
        let clean_contract = attempt_env.clean_contract;
        // Stage the antigravity-cli `model` field in
        // `~/.gemini/antigravity-cli/settings.json` before spawning `agy`,
        // and hold the RAII guard so the original contents are restored
        // when this function returns (#1620). For all other executors this
        // is `None` and a no-op.
        let _antigravity_guard = clean_contract
            .is_none()
            .then(|| executor.antigravity_settings_guard())
            .transpose()?
            .flatten();
        let (cmd, stdin_data) = if let Some(contract) = clean_contract {
            executor.build_clean_command(prompt, tool_state, contract)?
        } else {
            executor.build_command_with_git_push_allowed(
                prompt,
                tool_state,
                session,
                attempt_env.extra_env,
                options.subtree_pin.as_ref(),
                options.allow_git_push,
            )
        };

        let gemini_sandbox_plan = options
            .sandbox
            .filter(|_| executor.tool_name() == "gemini-cli")
            .map(|s| -> Result<_> {
                let mut isolation_plan = s.isolation_plan.clone();
                if let Some(env) = attempt_env.extra_env {
                    let runtime_home = gemini_runtime_home_from_env(env);
                    apply_gemini_sandbox_runtime_contract(
                        &mut isolation_plan,
                        Path::new(&session.project_path),
                        runtime_home.as_deref(),
                        env,
                        attempt_env.gemini_shared_npm_cache_raw_path,
                        attempt_env.gemini_shared_npm_cache_source,
                    )?;
                }
                Ok(isolation_plan)
            })
            .transpose()?;
        let isolation_plan = gemini_sandbox_plan
            .as_ref()
            .or_else(|| options.sandbox.map(|s| &s.isolation_plan));
        let best_effort = options.sandbox.is_some_and(|s| s.best_effort);
        let (tool_name, session_id) = options
            .sandbox
            .map(|s| (s.tool_name.as_str(), s.session_id.as_str()))
            .unwrap_or(("", ""));
        let diagnostic_session_id = if session_id.trim().is_empty() {
            session.meta_session_id.as_str()
        } else {
            session_id
        };
        let diagnostic_path = transport_meta::memory_soft_limit_diagnostic_path(
            Path::new(&session.project_path),
            diagnostic_session_id,
        );
        if let Some(path) = diagnostic_path.as_deref() {
            csa_resource::memory_monitor::clear_soft_limit_diagnostic(path);
        }

        let spawn_options = SpawnOptions {
            stdin_write_timeout: std::time::Duration::from_secs(
                options.stdin_write_timeout_seconds,
            ),
            keep_stdin_open: false,
            spool_max_bytes: options.output_spool_max_bytes,
            keep_rotated_spool: options.output_spool_keep_rotated,
            error_marker_scan_enabled: options.error_marker_scan_enabled,
        };
        let initial_response_timeout_seconds =
            Self::consume_resolved_transport_initial_response_timeout_seconds(
                options.initial_response_timeout,
            );
        let spawned = if let Some(contract) = clean_contract {
            csa_process::spawn_tool_sandboxed_in_environment(
                cmd,
                stdin_data.clone(),
                spawn_options,
                isolation_plan,
                tool_name,
                session_id,
                contract.environment(),
            )
            .await
        } else {
            spawn_tool_sandboxed(
                cmd,
                stdin_data.clone(),
                spawn_options,
                isolation_plan,
                tool_name,
                session_id,
            )
            .await
        };
        let (child, sandbox_handle) = match spawned {
            Ok(result) => result,
            Err(e) if best_effort && clean_contract.is_none() => {
                tracing::warn!(
                    "sandbox spawn failed in best-effort mode, falling back to unsandboxed: {e:#}"
                );
                let fallback_cmd = executor
                    .build_command_with_git_push_allowed(
                        prompt,
                        tool_state,
                        session,
                        attempt_env.extra_env,
                        options.subtree_pin.as_ref(),
                        options.allow_git_push,
                    )
                    .0;
                let child =
                    spawn_tool_with_options(fallback_cmd, stdin_data, spawn_options).await?;
                (child, csa_process::SandboxHandle::None)
            }
            Err(e) => return Err(e),
        };
        let child_pid = child.id();

        // Start memory monitor for legacy transport (mirrors ACP path).
        let memory_monitor = if let csa_process::SandboxHandle::Cgroup(ref guard) = sandbox_handle {
            isolation_plan.and_then(|plan| {
                transport_meta::start_memory_monitor(
                    guard.scope_name(),
                    child.id().unwrap_or(0),
                    plan,
                    std::time::Duration::from_secs(options.termination_grace_period_seconds),
                    diagnostic_path.clone(),
                )
            })
        } else {
            None
        };

        let mut execution = wait_and_capture_with_idle_timeout(
            child,
            options.stream_mode,
            std::time::Duration::from_secs(options.idle_timeout_seconds),
            std::time::Duration::from_secs(options.liveness_dead_seconds),
            std::time::Duration::from_secs(options.termination_grace_period_seconds),
            options.output_spool,
            spawn_options,
            initial_response_timeout_seconds.map(std::time::Duration::from_secs),
        )
        .await?;

        // Stop memory monitor before reading peak memory.
        if let Some(monitor) = memory_monitor {
            monitor.stop().await;
        }

        // Read peak memory from cgroup before sandbox_handle is dropped.
        if let csa_process::SandboxHandle::Cgroup(ref guard) = sandbox_handle {
            execution.peak_memory_mb = guard.memory_peak_mb();
            if let Some(peak) = execution.peak_memory_mb {
                tracing::info!(
                    tool = tool_name,
                    peak_memory_mb = peak,
                    "legacy transport: cgroup peak memory recorded"
                );
            }
        }
        // sandbox_handle is dropped here, cleaning up cgroup scope if applicable.
        if let Some(classification) = classify_codex_exec_initial_stall(
            executor,
            &execution,
            initial_response_timeout_seconds,
        ) {
            log_codex_exec_initial_stall(&classification, child_pid);
        }

        Ok(TransportResult {
            execution,
            provider_session_id: None,
            events: Vec::new(),
            metadata: Default::default(),
        })
    }
}

include!("transport_legacy_impl.rs");
