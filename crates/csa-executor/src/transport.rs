use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::executor::Executor;
use crate::lefthook_guard::sanitize_args_for_codex;
use crate::transport_gemini_retry::{
    gemini_auth_mode, gemini_inject_api_key_fallback, gemini_max_attempts,
    gemini_rate_limit_backoff, gemini_retry_model, gemini_should_use_api_key,
    is_gemini_rate_limited_error, is_gemini_rate_limited_result,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use csa_acp::SessionConfig;
use csa_process::{
    ExecutionResult, SpawnOptions, StreamMode, spawn_tool_sandboxed, spawn_tool_with_options,
    wait_and_capture_with_idle_timeout,
};
use csa_session::{
    new_session_id,
    state::{ContextStatus, Genealogy, MetaSessionState, SessionPhase, TaskContext, ToolState},
};

#[path = "transport_meta.rs"]
mod transport_meta;
pub use transport_meta::PeakMemoryContext;
use transport_meta::{build_summary, run_acp_sandboxed};
#[path = "transport_gemini_helpers.rs"]
mod transport_gemini_helpers;
#[cfg(test)]
use transport_gemini_helpers::format_gemini_retry_report;
use transport_gemini_helpers::{
    GeminiAcpInitFailureClassification, GeminiRetryPhase, annotate_gemini_retry_error,
    append_gemini_retry_report, apply_gemini_acp_initial_stall_summary,
    apply_gemini_legacy_initial_stall_summary, apply_gemini_mcp_warning_summary,
    apply_gemini_sandbox_runtime_env_overrides, classify_gemini_acp_init_failure,
    classify_gemini_acp_initial_stall, classify_gemini_legacy_initial_stall,
    classify_gemini_oauth_prompt_result, classify_join_error,
    ensure_gemini_runtime_home_writable_path, format_gemini_acp_init_failure,
    gemini_acp_initial_response_timeout_seconds, gemini_phase_desc,
    gemini_sandbox_runtime_env_overrides, is_gemini_acp_init_failure, is_gemini_mcp_issue_result,
    is_gemini_oauth_prompt_result,
};
pub use transport_gemini_helpers::{
    contains_gemini_oauth_prompt, normalize_gemini_prompt_text, strip_ansi_escape_sequences,
};
#[path = "transport_gemini_acp_runtime.rs"]
mod transport_gemini_acp_runtime;
use transport_gemini_acp_runtime::{
    gemini_runtime_home_from_env, prepare_gemini_acp_runtime, prepare_gemini_runtime_env,
    shared_npm_cache_dir,
};
#[path = "transport_gemini_mcp_diagnostic.rs"]
mod transport_gemini_mcp_diagnostic;
use transport_gemini_mcp_diagnostic::{
    McpInitDiagnostic, diagnose_mcp_init_failure, disable_mcp_servers_in_runtime,
    format_mcp_init_warning_summary, gemini_allow_degraded_mcp,
};
#[path = "transport_acp_crash_retry.rs"]
mod transport_acp_crash_retry;
use transport_acp_crash_retry::execute_with_crash_retry;
#[path = "transport_fork.rs"]
mod transport_fork;
pub use transport_fork::{ForkInfo, ForkMethod, ForkRequest};
#[path = "transport_factory.rs"]
mod transport_factory;
pub use transport_factory::{TransportFactory, TransportFactoryError, TransportMode};
#[path = "transport_acp_payload_debug.rs"]
mod transport_acp_payload_debug;
use transport_acp_payload_debug::{AcpPayloadDebugRequest, maybe_write_acp_payload_debug};
#[path = "transport_codex_exec_stall.rs"]
mod transport_codex_exec_stall;
#[path = "transport_legacy_codex_exec_stall.rs"]
mod transport_legacy_codex_exec_stall;
pub(crate) use transport_codex_exec_stall::resolve_execute_in_initial_response_timeout_seconds;
pub use transport_codex_exec_stall::resolve_initial_response_timeout;
pub use transport_codex_exec_stall::{
    CODEX_EXEC_INITIAL_STALL_REASON, DEFAULT_CODEX_INITIAL_RESPONSE_TIMEOUT_SECONDS,
    apply_codex_exec_initial_stall_summary, classify_codex_exec_initial_stall,
};
use transport_codex_exec_stall::{
    consume_resolved_execute_in_initial_response_timeout_seconds,
    consume_resolved_initial_response_timeout_seconds,
};
use transport_legacy_codex_exec_stall::{
    apply_and_maybe_retry_codex_exec_initial_stall, log_codex_exec_initial_stall,
};

#[path = "transport_types.rs"]
mod transport_types;
use transport_types::should_stream_acp_stdout_to_stderr;
pub use transport_types::{
    ResolvedTimeout, SandboxTransportConfig, TransportCapabilities, TransportOptions,
    TransportResult,
};

pub(crate) fn build_ephemeral_meta_session(work_dir: &Path) -> MetaSessionState {
    let now = Utc::now();
    MetaSessionState {
        meta_session_id: new_session_id(),
        description: None,
        project_path: work_dir.display().to_string(),
        branch: None,
        created_at: now,
        last_accessed: now,
        csa_version: None,
        genealogy: Genealogy::default(),
        tools: HashMap::new(),
        context_status: ContextStatus::default(),
        total_token_usage: None,
        phase: SessionPhase::Active,
        task_context: TaskContext::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        pre_session_porcelain: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        vcs_identity: None,
        identity_version: 1,
        fork_call_timestamps: Vec::new(),
    }
}

#[path = "transport_trait.rs"]
mod transport_trait;
pub use transport_trait::Transport;

#[derive(Debug, Clone)]
pub struct LegacyTransport {
    executor: Executor,
}

struct ExecuteInAttempt<'a> {
    executor: &'a Executor,
    prompt: &'a str,
    work_dir: &'a Path,
    extra_env: Option<&'a HashMap<String, String>>,
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

impl LegacyTransport {
    pub fn new(executor: Executor) -> Self {
        Self { executor }
    }

    fn should_retry_gemini_rate_limited(
        &self,
        execution: &ExecutionResult,
        attempt: u8,
        extra_env: Option<&HashMap<String, String>>,
    ) -> Option<Duration> {
        let max = gemini_max_attempts(extra_env);
        if !matches!(self.executor, Executor::GeminiCli { .. })
            || attempt >= max
            || !is_gemini_rate_limited_result(execution)
        {
            return None;
        }
        Some(gemini_rate_limit_backoff(attempt))
    }

    fn executor_for_attempt(&self, attempt: u8) -> Executor {
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
        let (cmd, stdin_data) = request.executor.build_execute_in_command(
            request.prompt,
            request.work_dir,
            request.extra_env,
        );
        let spawn_options = SpawnOptions {
            stdin_write_timeout: std::time::Duration::from_secs(
                csa_process::DEFAULT_STDIN_WRITE_TIMEOUT_SECS,
            ),
            keep_stdin_open: false,
            spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
            keep_rotated_spool: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
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

    fn consume_resolved_transport_initial_response_timeout_seconds(
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
        extra_env: Option<&HashMap<String, String>>,
        options: TransportOptions<'_>,
    ) -> Result<TransportResult> {
        let (cmd, stdin_data) = executor.build_command(prompt, tool_state, session, extra_env);

        let isolation_plan = options.sandbox.map(|s| &s.isolation_plan);
        let best_effort = options.sandbox.is_some_and(|s| s.best_effort);
        let (tool_name, session_id) = options
            .sandbox
            .map(|s| (s.tool_name.as_str(), s.session_id.as_str()))
            .unwrap_or(("", ""));

        let spawn_options = SpawnOptions {
            stdin_write_timeout: std::time::Duration::from_secs(
                options.stdin_write_timeout_seconds,
            ),
            keep_stdin_open: false,
            spool_max_bytes: options.output_spool_max_bytes,
            keep_rotated_spool: options.output_spool_keep_rotated,
        };
        let initial_response_timeout_seconds =
            Self::consume_resolved_transport_initial_response_timeout_seconds(
                options.initial_response_timeout,
            );
        let (child, sandbox_handle) = match spawn_tool_sandboxed(
            cmd,
            stdin_data.clone(),
            spawn_options,
            isolation_plan,
            tool_name,
            session_id,
        )
        .await
        {
            Ok(result) => result,
            Err(e) if best_effort => {
                tracing::warn!(
                    "sandbox spawn failed in best-effort mode, falling back to unsandboxed: {e:#}"
                );
                let fallback_cmd = executor
                    .build_command(prompt, tool_state, session, extra_env)
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

#[derive(Debug, Clone)]
pub struct AcpTransport {
    pub(crate) tool_name: String,
    acp_command: String,
    acp_args: Vec<String>,
    pub(crate) session_config: Option<SessionConfig>,
}

impl AcpTransport {
    pub fn new(tool_name: &str, session_config: Option<SessionConfig>) -> Self {
        let (cmd, args) = Self::acp_command_for_tool(tool_name);
        Self {
            tool_name: tool_name.to_string(),
            acp_command: cmd,
            acp_args: args,
            session_config,
        }
    }

    fn acp_command_for_tool(tool_name: &str) -> (String, Vec<String>) {
        // ACP adapters: @zed-industries/{codex,claude-code}-acp via npm;
        // gemini-cli has native ACP mode via `gemini --acp`.
        match tool_name {
            "claude-code" => ("claude-code-acp".into(), vec![]),
            "codex" => ("codex-acp".into(), vec![]),
            "gemini-cli" => ("gemini".into(), vec!["--acp".into()]),
            _ => (format!("{tool_name}-acp"), vec![]),
        }
    }
}

include!("transport_acp_spawn.rs");

impl AcpTransport {
    /// Execute a single ACP attempt with the given args and env.
    ///
    /// This is the core spawn_blocking logic extracted so the retry loop in
    /// `Transport::execute` can call it multiple times without duplicating
    /// the entire spawn/sandbox orchestration.
    async fn execute_acp_attempt(
        &self,
        prompt: &str,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        options: &TransportOptions<'_>,
        acp_args: &[String],
        resume_session_id: Option<&str>,
    ) -> Result<TransportResult> {
        let mut env = self.build_env(session, extra_env);
        let working_dir = Path::new(&session.project_path).to_path_buf();
        let system_prompt = Self::build_system_prompt(self.session_config.as_ref());
        let mut acp_command = self.acp_command.clone();
        let mut acp_args = acp_args.to_vec();
        let prompt = prompt.to_string();
        let resume_session_id = resume_session_id.map(String::from);
        let session_dir =
            csa_session::manager::get_session_dir(&working_dir, &session.meta_session_id).ok();

        let mut gemini_runtime_home = None;
        let shared_gemini_npm_cache = if self.tool_name == "gemini-cli" {
            let source_home = env
                .get("HOME")
                .cloned()
                .or_else(|| std::env::var("HOME").ok())
                .map(PathBuf::from);
            shared_npm_cache_dir(&env, source_home.as_deref())
        } else {
            None
        };
        if self.tool_name == "gemini-cli" {
            let launch = prepare_gemini_acp_runtime(
                &mut env,
                Some(working_dir.as_path()),
                session_dir.as_deref(),
                &session.meta_session_id,
                &acp_args,
            )?;
            acp_command = launch.command;
            acp_args = launch.args;
            gemini_runtime_home = gemini_runtime_home_from_env(&env);
        }
        if self.tool_name == "codex" {
            sanitize_args_for_codex(&mut acp_args);
        }
        let mut gemini_sandbox_env_overrides =
            (self.tool_name == "gemini-cli").then(|| gemini_sandbox_runtime_env_overrides(&env));

        let sandbox_plan = options.sandbox.map(|s| {
            let mut isolation_plan = s.isolation_plan.clone();
            if self.tool_name == "gemini-cli" {
                ensure_gemini_runtime_home_writable_path(
                    &mut isolation_plan,
                    gemini_runtime_home.as_deref(),
                );
                if let Some(shared_npm_cache) = shared_gemini_npm_cache.as_deref()
                    && !ensure_gemini_runtime_home_writable_path(
                        &mut isolation_plan,
                        Some(shared_npm_cache),
                    )
                {
                    env.remove("npm_config_cache");
                    if let Some(env_overrides) = gemini_sandbox_env_overrides.as_mut() {
                        env_overrides.remove("npm_config_cache");
                    }
                    tracing::warn!(
                        path = %shared_npm_cache.display(),
                        "shared npm cache dir not writable under sandbox; falling back to default per-session npm cache"
                    );
                }
                if let Some(ref env_overrides) = gemini_sandbox_env_overrides {
                    apply_gemini_sandbox_runtime_env_overrides(&mut isolation_plan, env_overrides);
                }
            }
            isolation_plan
        });
        let gemini_env_allowlist_applied = gemini_sandbox_env_overrides
            .as_ref()
            .map(|overrides| {
                let mut keys = overrides.keys().cloned().collect::<Vec<_>>();
                keys.sort();
                keys.join(",")
            })
            .unwrap_or_else(|| "none".to_string());
        let gemini_classification_env = (self.tool_name == "gemini-cli").then(|| env.clone());
        let sandbox_tool_name = options.sandbox.map(|s| s.tool_name.clone());
        let sandbox_session_id = options.sandbox.map(|s| s.session_id.clone());
        let sandbox_best_effort = options.sandbox.is_some_and(|s| s.best_effort);
        let idle_timeout_seconds = options.idle_timeout_seconds;
        let initial_response_timeout_seconds = if self.tool_name == "gemini-cli" {
            gemini_acp_initial_response_timeout_seconds(
                &self.tool_name,
                options.initial_response_timeout,
            )
        } else if self.tool_name == "codex" {
            consume_resolved_initial_response_timeout_seconds(options.initial_response_timeout)
        } else {
            None
        };
        let acp_init_timeout_seconds = options.acp_init_timeout_seconds;
        let termination_grace_period_seconds = options.termination_grace_period_seconds;
        let session_meta = Self::build_session_meta(
            options.setting_sources.as_deref(),
            self.session_config.as_ref(),
        );
        let acp_payload_debug_path = maybe_write_acp_payload_debug(AcpPayloadDebugRequest {
            env: &env,
            session_dir: session_dir.as_deref(),
            tool_name: &self.tool_name,
            command: &acp_command,
            args: &acp_args,
            working_dir: &working_dir,
            resume_session_id: resume_session_id.as_deref(),
            system_prompt: system_prompt.as_deref(),
            session_meta: session_meta.as_ref(),
            prompt: &prompt,
        });
        let stream_stdout_to_stderr =
            should_stream_acp_stdout_to_stderr(options.stream_mode, options.output_spool);
        let output_spool = options.output_spool.map(std::path::Path::to_path_buf);
        let output_spool_max_bytes = options.output_spool_max_bytes;
        let output_spool_keep_rotated = options.output_spool_keep_rotated;
        let spawn_request = AcpPromptRunRequest {
            tool_name: self.tool_name.clone(),
            acp_command,
            acp_args,
            prompt,
            working_dir,
            env,
            system_prompt,
            resume_session_id,
            session_meta,
            sandbox_plan,
            sandbox_tool_name,
            sandbox_session_id,
            sandbox_best_effort,
            idle_timeout_seconds,
            initial_response_timeout_seconds,
            acp_init_timeout_seconds,
            termination_grace_period_seconds,
            stream_stdout_to_stderr,
            output_spool,
            output_spool_max_bytes,
            output_spool_keep_rotated,
            acp_payload_debug_path,
            gemini_classification_env,
            gemini_env_allowlist_applied,
            memory_max_mb: options
                .sandbox
                .and_then(|sandbox| sandbox.isolation_plan.memory_max_mb),
        };

        let (output, gemini_warning_summary) = if self.tool_name == "gemini-cli" {
            let runtime_home = gemini_runtime_home
                .clone()
                .expect("gemini runtime home should exist for ACP execution");
            let path_override = spawn_request.env.get("PATH").map(std::ffi::OsString::from);
            let outcome = Self::execute_gemini_acp_with_degraded_mcp_retry(
                &runtime_home,
                path_override,
                gemini_allow_degraded_mcp(&spawn_request.env),
                || Self::run_acp_prompt(spawn_request.clone()),
                diagnose_mcp_init_failure,
                disable_mcp_servers_in_runtime,
                |error| {
                    let error_display = format!("{error:#}");
                    is_gemini_acp_init_failure(&error_display).then(|| {
                        classify_gemini_acp_init_failure(
                            &error_display,
                            spawn_request
                                .gemini_classification_env
                                .as_ref()
                                .expect("gemini classification env"),
                        )
                    })
                },
            )
            .await?;
            (outcome.value, outcome.warning_summary)
        } else {
            (Self::run_acp_prompt(spawn_request).await?, None)
        };
        let mut execution = ExecutionResult {
            summary: build_summary(&output.output, &output.stderr, output.exit_code),
            output: output.output,
            stderr_output: output.stderr,
            exit_code: output.exit_code,
            peak_memory_mb: output.peak_memory_mb,
        };
        if let Some(warning_summary) = gemini_warning_summary.as_deref() {
            apply_gemini_mcp_warning_summary(&mut execution, warning_summary);
        }
        if let Some(classification) = (self.tool_name == "gemini-cli")
            .then(|| {
                classify_gemini_acp_initial_stall(&execution, initial_response_timeout_seconds)
            })
            .flatten()
        {
            apply_gemini_acp_initial_stall_summary(&mut execution, &classification);
        }

        Ok(TransportResult {
            execution,
            provider_session_id: Some(output.session_id),
            events: output.events,
            metadata: output.metadata,
        })
    }
}

include!("transport_acp_impl.rs");

#[cfg(test)]
#[path = "transport_tests_mod.rs"]
mod tests;

#[cfg(test)]
#[path = "transport_lean_mode_tests.rs"]
mod lean_mode_tests;
