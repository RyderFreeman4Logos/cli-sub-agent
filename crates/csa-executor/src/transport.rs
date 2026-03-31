use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use crate::executor::Executor;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use csa_acp::{SessionConfig, SessionEvent};
use csa_core::gemini::{
    API_KEY_ENV as GEMINI_API_KEY_ENV, API_KEY_FALLBACK_ENV_KEY, AUTH_MODE_API_KEY,
    AUTH_MODE_ENV_KEY as GEMINI_AUTH_MODE_ENV_KEY, AUTH_MODE_OAUTH, NO_FLASH_FALLBACK_ENV_KEY,
    detect_rate_limit_pattern,
};
use csa_process::{
    ExecutionResult, SpawnOptions, StreamMode, spawn_tool_sandboxed, spawn_tool_with_options,
    wait_and_capture_with_idle_timeout,
};
use csa_resource::isolation_plan::IsolationPlan;
use csa_session::state::{MetaSessionState, ToolState};

#[path = "transport_meta.rs"]
mod transport_meta;
use transport_meta::{build_summary, run_acp_sandboxed};

#[path = "transport_fork.rs"]
mod transport_fork;
pub use transport_fork::{ForkInfo, ForkMethod, ForkRequest};

#[derive(Debug, Clone)]
pub struct SandboxTransportConfig {
    pub isolation_plan: IsolationPlan,
    pub tool_name: String,
    pub best_effort: bool,
    pub session_id: String,
}

#[derive(Debug, Clone)]
pub struct TransportOptions<'a> {
    pub stream_mode: StreamMode,
    pub idle_timeout_seconds: u64,
    pub initial_response_timeout_seconds: Option<u64>,
    pub liveness_dead_seconds: u64,
    pub stdin_write_timeout_seconds: u64,
    pub acp_init_timeout_seconds: u64,
    pub termination_grace_period_seconds: u64,
    pub output_spool: Option<&'a Path>,
    pub output_spool_max_bytes: u64,
    pub output_spool_keep_rotated: bool,
    pub setting_sources: Option<Vec<String>>,
    pub sandbox: Option<&'a SandboxTransportConfig>,
    /// When true, pass `--sandbox` to gemini-cli (prevents MCP server loading).
    pub gemini_sandbox: bool,
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn execute(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        options: TransportOptions<'_>,
    ) -> Result<TransportResult>;

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any;
}

#[derive(Debug, Clone)]
pub struct TransportResult {
    pub execution: ExecutionResult,
    pub provider_session_id: Option<String>,
    pub events: Vec<SessionEvent>,
    pub metadata: csa_acp::StreamingMetadata,
}
#[derive(Debug, Clone)]
pub struct LegacyTransport {
    executor: Executor,
}

const GEMINI_RATE_LIMIT_MAX_ATTEMPTS: u8 = 3;
const GEMINI_RATE_LIMIT_NO_FLASH_ATTEMPTS: u8 = 2;
#[cfg(test)]
const GEMINI_RATE_LIMIT_BASE_BACKOFF_MS: u64 = 10;
#[cfg(not(test))]
const GEMINI_RATE_LIMIT_BASE_BACKOFF_MS: u64 = 1_000;
const GEMINI_RATE_LIMIT_RETRY_MODEL_FIRST: &str = "gemini-3.1-pro-preview";
const GEMINI_RATE_LIMIT_RETRY_MODEL_SECOND: &str = "gemini-3-flash-preview";

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
        let max = if Self::is_no_flash(extra_env) {
            GEMINI_RATE_LIMIT_NO_FLASH_ATTEMPTS
        } else {
            GEMINI_RATE_LIMIT_MAX_ATTEMPTS
        };
        if !matches!(self.executor, Executor::GeminiCli { .. })
            || attempt >= max
            || !Self::is_gemini_rate_limited(execution)
        {
            return None;
        }
        Some(Self::gemini_rate_limit_backoff(attempt))
    }

    fn is_no_flash(extra_env: Option<&HashMap<String, String>>) -> bool {
        extra_env.is_some_and(|env| env.contains_key(NO_FLASH_FALLBACK_ENV_KEY))
    }

    fn is_gemini_rate_limited(execution: &ExecutionResult) -> bool {
        if execution.exit_code == 0 {
            return false;
        }
        detect_rate_limit_pattern(&format!(
            "{}\n{}",
            execution.stderr_output, execution.output
        ))
        .is_some()
    }

    fn gemini_rate_limit_backoff(attempt: u8) -> Duration {
        let exponent = u32::from(attempt.saturating_sub(1));
        let multiplier = 1u64.checked_shl(exponent).unwrap_or(u64::MAX);
        Duration::from_millis(GEMINI_RATE_LIMIT_BASE_BACKOFF_MS.saturating_mul(multiplier))
    }

    fn gemini_rate_limit_retry_model(attempt: u8) -> Option<&'static str> {
        match attempt {
            2 => Some(GEMINI_RATE_LIMIT_RETRY_MODEL_FIRST),
            3 => Some(GEMINI_RATE_LIMIT_RETRY_MODEL_SECOND),
            _ => None,
        }
    }

    fn gemini_auth_mode(extra_env: Option<&HashMap<String, String>>) -> Option<&str> {
        extra_env
            .and_then(|env| env.get(GEMINI_AUTH_MODE_ENV_KEY))
            .map(String::as_str)
    }

    /// Build extra_env with GEMINI_API_KEY injected from the fallback key.
    /// Returns None if no fallback key is available.
    fn inject_api_key_fallback(
        extra_env: Option<&HashMap<String, String>>,
    ) -> Option<HashMap<String, String>> {
        if Self::gemini_auth_mode(extra_env) != Some(AUTH_MODE_OAUTH) {
            return None;
        }
        let fallback_key = extra_env?.get(API_KEY_FALLBACK_ENV_KEY)?;
        let mut env = extra_env.cloned().unwrap_or_default();
        env.insert(GEMINI_API_KEY_ENV.to_string(), fallback_key.clone());
        env.insert(
            GEMINI_AUTH_MODE_ENV_KEY.to_string(),
            AUTH_MODE_API_KEY.to_string(),
        );
        env.remove(API_KEY_FALLBACK_ENV_KEY);
        Some(env)
    }

    fn executor_for_attempt(&self, attempt: u8) -> Executor {
        match &self.executor {
            Executor::GeminiCli {
                thinking_budget, ..
            } => {
                if let Some(model) = Self::gemini_rate_limit_retry_model(attempt) {
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
        executor: &Executor,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        stream_mode: StreamMode,
        idle_timeout_seconds: u64,
    ) -> Result<TransportResult> {
        let (cmd, stdin_data) = executor.build_execute_in_command(prompt, work_dir, extra_env);
        let spawn_options = SpawnOptions {
            stdin_write_timeout: std::time::Duration::from_secs(
                csa_process::DEFAULT_STDIN_WRITE_TIMEOUT_SECS,
            ),
            keep_stdin_open: false,
            spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
            keep_rotated_spool: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
        };
        let child = spawn_tool_with_options(cmd, stdin_data, spawn_options).await?;
        let execution = wait_and_capture_with_idle_timeout(
            child,
            stream_mode,
            std::time::Duration::from_secs(idle_timeout_seconds),
            std::time::Duration::from_secs(csa_process::DEFAULT_LIVENESS_DEAD_SECS),
            std::time::Duration::from_secs(csa_process::DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
            None,
            spawn_options,
            None,
        )
        .await?;
        Ok(TransportResult {
            execution,
            provider_session_id: None,
            events: Vec::new(),
            metadata: Default::default(),
        })
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
        let (mut cmd, stdin_data) = executor.build_command(prompt, tool_state, session, extra_env);
        if options.gemini_sandbox && matches!(executor, Executor::GeminiCli { .. }) {
            cmd.arg("--sandbox");
        }

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
        let (child, _sandbox_handle) = match spawn_tool_sandboxed(
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
                let mut fallback_cmd = executor
                    .build_command(prompt, tool_state, session, extra_env)
                    .0;
                if options.gemini_sandbox && matches!(executor, Executor::GeminiCli { .. }) {
                    fallback_cmd.arg("--sandbox");
                }
                let child =
                    spawn_tool_with_options(fallback_cmd, stdin_data, spawn_options).await?;
                (child, csa_process::SandboxHandle::None)
            }
            Err(e) => return Err(e),
        };

        let execution = wait_and_capture_with_idle_timeout(
            child,
            options.stream_mode,
            std::time::Duration::from_secs(options.idle_timeout_seconds),
            std::time::Duration::from_secs(options.liveness_dead_seconds),
            std::time::Duration::from_secs(options.termination_grace_period_seconds),
            options.output_spool,
            spawn_options,
            options
                .initial_response_timeout_seconds
                .map(std::time::Duration::from_secs),
        )
        .await?;

        // _sandbox_handle is kept alive until here, then dropped (cleanup).

        Ok(TransportResult {
            execution,
            provider_session_id: None,
            events: Vec::new(),
            metadata: Default::default(),
        })
    }

    pub async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        stream_mode: StreamMode,
        idle_timeout_seconds: u64,
    ) -> Result<TransportResult> {
        let mut attempt = 1u8;
        loop {
            let executor = self.executor_for_attempt(attempt);
            let result = self
                .execute_in_single_attempt(
                    &executor,
                    prompt,
                    work_dir,
                    extra_env,
                    stream_mode,
                    idle_timeout_seconds,
                )
                .await?;
            if let Some(backoff) =
                self.should_retry_gemini_rate_limited(&result.execution, attempt, extra_env)
            {
                tracing::debug!(attempt, "gemini-cli rate limit; retrying with model switch");
                tokio::time::sleep(backoff).await;
                attempt = attempt.saturating_add(1);
                continue;
            }
            // API key fallback: all model retries exhausted, still quota error.
            if Self::is_gemini_rate_limited(&result.execution)
                && let Some(env_with_key) = Self::inject_api_key_fallback(extra_env)
            {
                tracing::info!("gemini-cli quota exhausted; falling back to API key auth");
                return self
                    .execute_in_single_attempt(
                        &self.executor,
                        prompt,
                        work_dir,
                        Some(&env_with_key),
                        stream_mode,
                        idle_timeout_seconds,
                    )
                    .await;
            }
            return Ok(result);
        }
    }
}

#[async_trait]
impl Transport for LegacyTransport {
    async fn execute(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        options: TransportOptions<'_>,
    ) -> Result<TransportResult> {
        let mut attempt = 1u8;
        loop {
            let executor = self.executor_for_attempt(attempt);
            let result = self
                .execute_single_attempt(
                    &executor,
                    prompt,
                    tool_state,
                    session,
                    extra_env,
                    options.clone(),
                )
                .await?;
            if let Some(backoff) =
                self.should_retry_gemini_rate_limited(&result.execution, attempt, extra_env)
            {
                tracing::debug!(attempt, "gemini-cli rate limit; retrying with model switch");
                tokio::time::sleep(backoff).await;
                attempt = attempt.saturating_add(1);
                continue;
            }
            // API key fallback: all model retries exhausted, still quota error.
            if Self::is_gemini_rate_limited(&result.execution)
                && let Some(env_with_key) = Self::inject_api_key_fallback(extra_env)
            {
                tracing::info!("gemini-cli quota exhausted; falling back to API key auth");
                return self
                    .execute_single_attempt(
                        &self.executor,
                        prompt,
                        tool_state,
                        session,
                        Some(&env_with_key),
                        options,
                    )
                    .await;
            }
            return Ok(result);
        }
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

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
        // ACP adapters: @zed-industries/{codex,claude-code}-acp via npm.
        match tool_name {
            "claude-code" => ("claude-code-acp".into(), vec![]),
            "codex" => ("codex-acp".into(), vec![]),
            _ => (format!("{tool_name}-acp"), vec![]),
        }
    }
}

#[async_trait]
impl Transport for AcpTransport {
    #[tracing::instrument(skip_all, fields(tool = %self.tool_name))]
    async fn execute(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        options: TransportOptions<'_>,
    ) -> Result<TransportResult> {
        let env = self.build_env(session, extra_env);
        let working_dir = Path::new(&session.project_path).to_path_buf();
        let system_prompt = Self::build_system_prompt(self.session_config.as_ref());
        let acp_command = self.acp_command.clone();
        let acp_args = self.acp_args.clone();
        let prompt = prompt.to_string();
        let resume_session_id = tool_state.and_then(|s| s.provider_session_id.clone());
        if let Some(session_id) = resume_session_id.as_deref() {
            tracing::debug!(session_id, "resuming ACP session from tool state");
        }

        let sandbox_plan = options.sandbox.map(|s| s.isolation_plan.clone());
        let sandbox_tool_name = options.sandbox.map(|s| s.tool_name.clone());
        let sandbox_session_id = options.sandbox.map(|s| s.session_id.clone());
        let sandbox_best_effort = options.sandbox.is_some_and(|s| s.best_effort);
        let idle_timeout_seconds = options.idle_timeout_seconds;
        let initial_response_timeout_seconds = options.initial_response_timeout_seconds;
        let acp_init_timeout_seconds = options.acp_init_timeout_seconds;
        let termination_grace_period_seconds = options.termination_grace_period_seconds;
        let session_meta = Self::build_session_meta(
            options.setting_sources.as_deref(),
            self.session_config.as_ref(),
        );
        let stream_stdout_to_stderr = options.stream_mode != StreamMode::BufferOnly;
        let output_spool = options.output_spool.map(std::path::Path::to_path_buf);
        let output_spool_max_bytes = options.output_spool_max_bytes;
        let output_spool_keep_rotated = options.output_spool_keep_rotated;

        let output =
            tokio::task::spawn_blocking(move || -> Result<csa_acp::transport::AcpOutput> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| anyhow!("failed to build ACP runtime: {e}"))?;

                if let Some(ref plan) = sandbox_plan {
                    let tool_name = sandbox_tool_name.as_deref().unwrap_or("");
                    let sess_id = sandbox_session_id.as_deref().unwrap_or("");
                    match rt.block_on(run_acp_sandboxed(
                        &acp_command,
                        &acp_args,
                        &working_dir,
                        &env,
                        system_prompt.as_deref(),
                        resume_session_id.as_deref(),
                        session_meta.clone(),
                        &prompt,
                        std::time::Duration::from_secs(idle_timeout_seconds),
                        initial_response_timeout_seconds
                            .map(std::time::Duration::from_secs),
                        std::time::Duration::from_secs(acp_init_timeout_seconds),
                        std::time::Duration::from_secs(termination_grace_period_seconds),
                        plan,
                        tool_name,
                        sess_id,
                        stream_stdout_to_stderr,
                        output_spool.as_deref(),
                        output_spool_max_bytes,
                        output_spool_keep_rotated,
                    )) {
                        Ok(output) => Ok(output),
                        Err(e) if sandbox_best_effort => {
                            tracing::warn!(
                                "ACP sandbox spawn failed in best-effort mode, falling back to unsandboxed: {e}"
                            );
                            rt.block_on(csa_acp::transport::run_prompt_with_io(
                                &acp_command,
                                &acp_args,
                                &working_dir,
                                &env,
                                csa_acp::transport::AcpSessionStart {
                                    system_prompt: system_prompt.as_deref(),
                                    resume_session_id: resume_session_id.as_deref(),
                                    meta: session_meta.clone(),
                                    ..Default::default()
                                },
                                &prompt,
                                csa_acp::transport::AcpRunOptions {
                                    idle_timeout: std::time::Duration::from_secs(
                                        idle_timeout_seconds,
                                    ),
                                    initial_response_timeout: initial_response_timeout_seconds
                                        .map(std::time::Duration::from_secs),
                                    init_timeout: std::time::Duration::from_secs(
                                        acp_init_timeout_seconds,
                                    ),
                                    termination_grace_period: std::time::Duration::from_secs(
                                        termination_grace_period_seconds,
                                    ),
                                    io: csa_acp::transport::AcpOutputIoOptions {
                                        stream_stdout_to_stderr,
                                        output_spool: output_spool.as_deref(),
                                        spool_max_bytes: output_spool_max_bytes,
                                        keep_rotated_spool: output_spool_keep_rotated,
                                    },
                                },
                            ))
                            .map_err(|e| anyhow!("ACP transport (unsandboxed fallback) failed: {e}"))
                        }
                        Err(e) => Err(anyhow!("ACP transport (sandboxed) failed: {e}")),
                    }
                } else {
                    rt.block_on(csa_acp::transport::run_prompt_with_io(
                        &acp_command,
                        &acp_args,
                        &working_dir,
                        &env,
                        csa_acp::transport::AcpSessionStart {
                            system_prompt: system_prompt.as_deref(),
                            resume_session_id: resume_session_id.as_deref(),
                            meta: session_meta.clone(),
                            ..Default::default()
                        },
                        &prompt,
                        csa_acp::transport::AcpRunOptions {
                            idle_timeout: std::time::Duration::from_secs(idle_timeout_seconds),
                            initial_response_timeout: initial_response_timeout_seconds
                                .map(std::time::Duration::from_secs),
                            init_timeout: std::time::Duration::from_secs(
                                acp_init_timeout_seconds,
                            ),
                            termination_grace_period: std::time::Duration::from_secs(
                                termination_grace_period_seconds,
                            ),
                            io: csa_acp::transport::AcpOutputIoOptions {
                                stream_stdout_to_stderr,
                                output_spool: output_spool.as_deref(),
                                spool_max_bytes: output_spool_max_bytes,
                                keep_rotated_spool: output_spool_keep_rotated,
                            },
                        },
                    ))
                    .map_err(|e| anyhow!("ACP transport failed: {e}"))
                }
            })
            .await
            .map_err(classify_join_error)??;

        let execution = ExecutionResult {
            summary: build_summary(&output.output, &output.stderr, output.exit_code),
            output: output.output,
            stderr_output: output.stderr,
            exit_code: output.exit_code,
        };

        Ok(TransportResult {
            execution,
            provider_session_id: Some(output.session_id),
            events: output.events,
            metadata: output.metadata,
        })
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Convert a `tokio::task::JoinError` into a descriptive `anyhow::Error`.
///
/// Broken-pipe panics (from `eprintln!` after the tool process closes stderr)
/// are rewritten into a clean message that mentions the tool died, instead of
/// surfacing the raw tokio panic trace.
fn classify_join_error(e: tokio::task::JoinError) -> anyhow::Error {
    if e.is_panic() {
        let msg = match e.into_panic().downcast::<String>() {
            Ok(s) => *s,
            Err(any) => match any.downcast::<&str>() {
                Ok(s) => s.to_string(),
                Err(_) => "unknown panic".to_string(),
            },
        };
        if msg.contains("Broken pipe") || msg.contains("os error 32") {
            return anyhow!(
                "ACP transport: tool process terminated unexpectedly (broken pipe on stderr)"
            );
        }
        anyhow!("ACP transport: task panicked: {msg}")
    } else {
        anyhow!("ACP transport: task cancelled: {e}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    Legacy,
    Acp,
    OpenaiCompat,
}

pub struct TransportFactory;

impl TransportFactory {
    pub fn mode_for_tool(tool_name: &str) -> TransportMode {
        match tool_name {
            "claude-code" | "codex" => TransportMode::Acp,
            "openai-compat" => TransportMode::OpenaiCompat,
            _ => TransportMode::Legacy,
        }
    }

    pub fn create(
        executor: &Executor,
        session_config: Option<SessionConfig>,
    ) -> Box<dyn Transport> {
        match Self::mode_for_tool(executor.tool_name()) {
            TransportMode::Legacy => Box::new(LegacyTransport::new(executor.clone())),
            TransportMode::Acp => Box::new(AcpTransport::new(executor.tool_name(), session_config)),
            TransportMode::OpenaiCompat => {
                let default_model = if let Executor::OpenaiCompat { model_override, .. } = executor
                {
                    model_override.clone()
                } else {
                    None
                };
                Box::new(crate::transport_openai_compat::OpenaiCompatTransport::new(
                    default_model,
                ))
            }
        }
    }

    /// Create an OpenAI-compat transport with explicit config.
    pub fn create_openai_compat(
        config: crate::transport_openai_compat::OpenaiCompatConfig,
    ) -> Box<dyn Transport> {
        Box::new(crate::transport_openai_compat::OpenaiCompatTransport::with_config(config))
    }
}

#[cfg(test)]
mod tests {
    use csa_acp::SessionConfig;

    use super::*;

    #[test]
    fn test_transport_factory_create_routes_tools_to_expected_transport() {
        let legacy_tools = vec![
            Executor::GeminiCli {
                model_override: None,
                thinking_budget: None,
            },
            Executor::Opencode {
                model_override: None,
                agent: None,
                thinking_budget: None,
            },
        ];
        for executor in legacy_tools {
            let transport = TransportFactory::create(&executor, None);
            assert!(
                transport.as_ref().as_any().is::<LegacyTransport>(),
                "Expected LegacyTransport for {}",
                executor.tool_name()
            );
        }

        let acp_tools = vec![
            Executor::Codex {
                model_override: None,
                thinking_budget: None,
            },
            Executor::ClaudeCode {
                model_override: None,
                thinking_budget: None,
            },
        ];
        for executor in acp_tools {
            let transport = TransportFactory::create(&executor, Some(SessionConfig::default()));
            assert!(
                transport.as_ref().as_any().is::<AcpTransport>(),
                "Expected AcpTransport for {}",
                executor.tool_name()
            );
        }
    }

    #[test]
    fn test_transport_factory_create_preserves_session_config_for_acp_transport() {
        let executor = Executor::Codex {
            model_override: None,
            thinking_budget: None,
        };
        let session_config = SessionConfig {
            no_load: vec!["skills/foo".to_string()],
            extra_load: vec!["skills/bar".to_string()],
            tier: Some("tier-2".to_string()),
            models: vec!["codex/openai/o3/medium".to_string()],
            mcp_servers: Vec::new(),
            mcp_proxy_socket: None,
        };

        let transport = TransportFactory::create(&executor, Some(session_config.clone()));
        let acp = transport
            .as_ref()
            .as_any()
            .downcast_ref::<AcpTransport>()
            .expect("expected AcpTransport");

        assert_eq!(acp.session_config, Some(session_config));
    }

    #[test]
    fn test_legacy_transport_construction_from_executor() {
        let executor = Executor::Opencode {
            model_override: Some("model".to_string()),
            agent: Some("coder".to_string()),
            thinking_budget: None,
        };
        let transport = LegacyTransport::new(executor.clone());

        assert_eq!(transport.executor.tool_name(), executor.tool_name());
        assert_eq!(
            transport.executor.executable_name(),
            executor.executable_name()
        );
    }

    #[test]
    fn test_acp_command_for_tool_mappings() {
        assert_eq!(
            AcpTransport::acp_command_for_tool("claude-code"),
            ("claude-code-acp".to_string(), vec![])
        );
        assert_eq!(
            AcpTransport::acp_command_for_tool("codex"),
            ("codex-acp".to_string(), vec![])
        );
        // Unknown tools get "{name}-acp" convention
        assert_eq!(
            AcpTransport::acp_command_for_tool("opencode"),
            ("opencode-acp".to_string(), vec![])
        );
        assert_eq!(
            AcpTransport::acp_command_for_tool("gemini-cli"),
            ("gemini-cli-acp".to_string(), vec![])
        );
    }

    include!("transport_tests_tail.rs");
}

#[cfg(test)]
#[path = "transport_lean_mode_tests.rs"]
mod lean_mode_tests;
