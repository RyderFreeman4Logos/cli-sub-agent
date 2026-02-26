use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::executor::Executor;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use csa_acp::{SessionConfig, SessionEvent};
use csa_process::{
    ExecutionResult, SpawnOptions, StreamMode, spawn_tool_sandboxed, spawn_tool_with_options,
    wait_and_capture_with_idle_timeout,
};
use csa_resource::cgroup::SandboxConfig;
use csa_session::state::{MetaSessionState, ToolState};

#[path = "transport_meta.rs"]
mod transport_meta;
use transport_meta::{build_summary, run_acp_sandboxed};

#[derive(Debug, Clone)]
pub struct SandboxTransportConfig {
    pub config: SandboxConfig,
    pub tool_name: String,
    pub best_effort: bool,
    pub session_id: String,
}

#[derive(Debug, Clone)]
pub struct TransportOptions<'a> {
    pub stream_mode: StreamMode,
    pub idle_timeout_seconds: u64,
    pub liveness_dead_seconds: u64,
    pub stdin_write_timeout_seconds: u64,
    pub acp_init_timeout_seconds: u64,
    pub termination_grace_period_seconds: u64,
    pub output_spool: Option<&'a Path>,
    pub setting_sources: Option<Vec<String>>,
    pub sandbox: Option<&'a SandboxTransportConfig>,
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
}
#[derive(Debug, Clone)]
pub struct LegacyTransport {
    executor: Executor,
}
impl LegacyTransport {
    pub fn new(executor: Executor) -> Self {
        Self { executor }
    }

    pub async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        stream_mode: StreamMode,
        idle_timeout_seconds: u64,
    ) -> Result<TransportResult> {
        let (cmd, stdin_data) = self
            .executor
            .build_execute_in_command(prompt, work_dir, extra_env);
        let child = spawn_tool_with_options(
            cmd,
            stdin_data,
            SpawnOptions {
                stdin_write_timeout: std::time::Duration::from_secs(
                    csa_process::DEFAULT_STDIN_WRITE_TIMEOUT_SECS,
                ),
                keep_stdin_open: false,
            },
        )
        .await?;
        let execution = wait_and_capture_with_idle_timeout(
            child,
            stream_mode,
            std::time::Duration::from_secs(idle_timeout_seconds),
            std::time::Duration::from_secs(csa_process::DEFAULT_LIVENESS_DEAD_SECS),
            std::time::Duration::from_secs(csa_process::DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
            None,
        )
        .await?;
        Ok(TransportResult {
            execution,
            provider_session_id: None,
            events: Vec::new(),
        })
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
        let (cmd, stdin_data) = self
            .executor
            .build_command(prompt, tool_state, session, extra_env);

        let sandbox_cfg = options.sandbox.map(|s| &s.config);
        let best_effort = options.sandbox.is_some_and(|s| s.best_effort);
        let (tool_name, session_id) = options
            .sandbox
            .map(|s| (s.tool_name.as_str(), s.session_id.as_str()))
            .unwrap_or(("", ""));

        let (child, _sandbox_handle) = match spawn_tool_sandboxed(
            cmd,
            stdin_data.clone(),
            SpawnOptions {
                stdin_write_timeout: std::time::Duration::from_secs(
                    options.stdin_write_timeout_seconds,
                ),
                keep_stdin_open: false,
            },
            sandbox_cfg,
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
                let child = spawn_tool_with_options(
                    self.executor
                        .build_command(prompt, tool_state, session, extra_env)
                        .0,
                    stdin_data,
                    SpawnOptions {
                        stdin_write_timeout: std::time::Duration::from_secs(
                            options.stdin_write_timeout_seconds,
                        ),
                        keep_stdin_open: false,
                    },
                )
                .await?;
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
        )
        .await?;

        // _sandbox_handle is kept alive until here, then dropped (cleanup).

        Ok(TransportResult {
            execution,
            provider_session_id: None,
            events: Vec::new(),
        })
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
        // ACP adapters are standalone binaries from Zed Industries:
        //   npm: @zed-industries/codex-acp, @zed-industries/claude-code-acp
        // They bridge the tool's SDK to ACP protocol over stdio.
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

        let sandbox_config = options.sandbox.map(|s| s.config.clone());
        let sandbox_tool_name = options.sandbox.map(|s| s.tool_name.clone());
        let sandbox_session_id = options.sandbox.map(|s| s.session_id.clone());
        let sandbox_best_effort = options.sandbox.is_some_and(|s| s.best_effort);
        let idle_timeout_seconds = options.idle_timeout_seconds;
        let acp_init_timeout_seconds = options.acp_init_timeout_seconds;
        let termination_grace_period_seconds = options.termination_grace_period_seconds;
        let session_meta = Self::build_session_meta(
            options.setting_sources.as_deref(),
            self.session_config.as_ref(),
        );
        let stream_stdout_to_stderr = options.stream_mode != StreamMode::BufferOnly;
        let output_spool = options.output_spool.map(std::path::Path::to_path_buf);

        let output =
            tokio::task::spawn_blocking(move || -> Result<csa_acp::transport::AcpOutput> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| anyhow!("failed to build ACP runtime: {e}"))?;

                if let Some(ref cfg) = sandbox_config {
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
                        std::time::Duration::from_secs(acp_init_timeout_seconds),
                        std::time::Duration::from_secs(termination_grace_period_seconds),
                        cfg,
                        tool_name,
                        sess_id,
                        stream_stdout_to_stderr,
                        output_spool.as_deref(),
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
                                    init_timeout: std::time::Duration::from_secs(
                                        acp_init_timeout_seconds,
                                    ),
                                    termination_grace_period: std::time::Duration::from_secs(
                                        termination_grace_period_seconds,
                                    ),
                                    io: csa_acp::transport::AcpOutputIoOptions {
                                        stream_stdout_to_stderr,
                                        output_spool: output_spool.as_deref(),
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
                            init_timeout: std::time::Duration::from_secs(
                                acp_init_timeout_seconds,
                            ),
                            termination_grace_period: std::time::Duration::from_secs(
                                termination_grace_period_seconds,
                            ),
                            io: csa_acp::transport::AcpOutputIoOptions {
                                stream_stdout_to_stderr,
                                output_spool: output_spool.as_deref(),
                            },
                        },
                    ))
                    .map_err(|e| anyhow!("ACP transport failed: {e}"))
                }
            })
            .await
            .map_err(|e| anyhow!("ACP transport join error: {e}"))??;

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
        })
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    Legacy,
    Acp,
}

pub struct TransportFactory;

impl TransportFactory {
    pub fn mode_for_tool(tool_name: &str) -> TransportMode {
        if matches!(tool_name, "claude-code" | "codex") {
            TransportMode::Acp
        } else {
            TransportMode::Legacy
        }
    }

    pub fn create(
        executor: &Executor,
        session_config: Option<SessionConfig>,
    ) -> Box<dyn Transport> {
        match Self::mode_for_tool(executor.tool_name()) {
            TransportMode::Legacy => Box::new(LegacyTransport::new(executor.clone())),
            TransportMode::Acp => Box::new(AcpTransport::new(executor.tool_name(), session_config)),
        }
    }
}

/// Which fork strategy was used for a session fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkMethod {
    /// Claude Code CLI `--fork-session` (provider-level fork).
    Native,
    /// Context summary injection for tools without native fork support.
    Soft,
}

/// Result of a transport-level session fork attempt.
#[derive(Debug, Clone)]
pub struct ForkInfo {
    /// Whether the fork succeeded.
    pub success: bool,
    /// Which fork method was used.
    pub method: ForkMethod,
    /// New provider-level session ID (Native forks only).
    pub new_session_id: Option<String>,
    /// Human-readable notes (e.g. error details or context summary path).
    pub notes: Option<String>,
}

/// Options for a transport-level fork request.
#[derive(Debug, Clone)]
pub struct ForkRequest {
    /// The tool name.
    pub tool_name: String,
    /// Pre-computed fork method. When set, `fork_session` uses this instead of
    /// re-deriving from `tool_name`. This is essential for cross-tool forks where
    /// `resolve_fork` determines `ForkMethod::Soft` but the target tool would
    /// otherwise select `Native`.
    pub fork_method: Option<ForkMethod>,
    /// Provider session ID of the parent (required for Native fork).
    pub provider_session_id: Option<String>,
    /// Whether Codex PTY native fork should auto-accept trust prompts.
    pub codex_auto_trust: bool,
    /// CSA session ID of the parent (used for Soft fork context loading).
    pub parent_csa_session_id: String,
    /// Directory of the parent session (used for Soft fork to read result/output).
    pub parent_session_dir: PathBuf,
    /// Working directory for CLI fork commands.
    pub working_dir: PathBuf,
    /// Timeout for the CLI fork subprocess.
    pub timeout: Duration,
}

impl TransportFactory {
    /// Determine which fork method applies for a given tool.
    pub fn fork_method_for_tool(tool_name: &str) -> ForkMethod {
        if tool_name == "claude-code" {
            ForkMethod::Native
        } else if tool_name == "codex" {
            #[cfg(feature = "codex-pty-fork")]
            {
                ForkMethod::Native
            }
            #[cfg(not(feature = "codex-pty-fork"))]
            {
                ForkMethod::Soft
            }
        } else {
            ForkMethod::Soft
        }
    }

    /// Fork a session via the appropriate transport-level mechanism.
    ///
    /// - `claude-code`: Native fork via `claude --fork-session` CLI.
    /// - All others: Soft fork via context summary injection from parent session.
    pub async fn fork_session(request: &ForkRequest) -> ForkInfo {
        let method = request
            .fork_method
            .unwrap_or_else(|| Self::fork_method_for_tool(&request.tool_name));
        match method {
            ForkMethod::Native => Self::fork_native(request).await,
            ForkMethod::Soft => Self::fork_soft(request),
        }
    }

    async fn fork_native(request: &ForkRequest) -> ForkInfo {
        if request.tool_name == "codex" {
            return Self::fork_codex_via_pty(request).await;
        }

        let Some(provider_session_id) = &request.provider_session_id else {
            return ForkInfo {
                success: false,
                method: ForkMethod::Native,
                new_session_id: None,
                notes: Some(
                    "Native fork requires provider_session_id, but none was provided".to_string(),
                ),
            };
        };

        match csa_acp::fork_session_via_cli(
            provider_session_id,
            &request.working_dir,
            request.timeout,
        )
        .await
        {
            Ok(result) => ForkInfo {
                success: true,
                method: ForkMethod::Native,
                new_session_id: Some(result.session_id),
                notes: None,
            },
            Err(e) => ForkInfo {
                success: false,
                method: ForkMethod::Native,
                new_session_id: None,
                notes: Some(format!("Native fork failed: {e}")),
            },
        }
    }

    #[cfg(feature = "codex-pty-fork")]
    async fn fork_codex_via_pty(request: &ForkRequest) -> ForkInfo {
        use csa_process::pty_fork::{PtyForkConfig, PtyForkResult, fork_codex_session};

        let Some(parent_provider_session_id) = request.provider_session_id.as_deref() else {
            let reason = "codex native fork requires provider_session_id";
            tracing::warn!(tool = %request.tool_name, reason, "native fork degraded to soft fork");
            return Self::fork_soft_with_reason(request, reason);
        };

        let config = PtyForkConfig {
            codex_auto_trust: request.codex_auto_trust,
            ..PtyForkConfig::default()
        };
        match fork_codex_session(parent_provider_session_id, Path::new("codex"), &config).await {
            Ok(PtyForkResult::Success { child_session_id }) => ForkInfo {
                success: true,
                method: ForkMethod::Native,
                new_session_id: Some(child_session_id),
                notes: None,
            },
            Ok(PtyForkResult::Degraded { reason }) => {
                tracing::warn!(
                    tool = %request.tool_name,
                    reason = %reason,
                    "native fork degraded to soft fork"
                );
                Self::fork_soft_with_reason(request, &reason)
            }
            Ok(PtyForkResult::Failed { error }) => {
                tracing::warn!(
                    tool = %request.tool_name,
                    error = %error,
                    "native fork failed; falling back to soft fork"
                );
                Self::fork_soft_with_reason(request, &format!("Native codex fork failed: {error}"))
            }
            Err(e) => {
                tracing::warn!(
                    tool = %request.tool_name,
                    error = %e,
                    "native fork errored; falling back to soft fork"
                );
                Self::fork_soft_with_reason(
                    request,
                    &format!("Native codex fork errored unexpectedly: {e}"),
                )
            }
        }
    }

    #[cfg(not(feature = "codex-pty-fork"))]
    async fn fork_codex_via_pty(request: &ForkRequest) -> ForkInfo {
        Self::fork_soft_with_reason(
            request,
            "Native codex fork unavailable: feature `codex-pty-fork` is disabled",
        )
    }

    fn fork_soft_with_reason(request: &ForkRequest, native_reason: &str) -> ForkInfo {
        let mut soft = Self::fork_soft(request);
        let soft_note = soft.notes.take().unwrap_or_default();
        soft.notes = Some(if soft_note.is_empty() {
            format!("Native codex fork degraded: {native_reason}")
        } else {
            format!("Native codex fork degraded: {native_reason}; {soft_note}")
        });
        soft
    }

    fn fork_soft(request: &ForkRequest) -> ForkInfo {
        match csa_session::soft_fork_session(
            &request.parent_session_dir,
            &request.parent_csa_session_id,
        ) {
            Ok(ctx) => ForkInfo {
                success: true,
                method: ForkMethod::Soft,
                new_session_id: None,
                notes: Some(format!(
                    "Soft fork context ({} chars) ready for injection",
                    ctx.context_summary.len()
                )),
            },
            Err(e) => ForkInfo {
                success: false,
                method: ForkMethod::Soft,
                new_session_id: None,
                notes: Some(format!("Soft fork failed: {e}")),
            },
        }
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

    #[test]
    fn test_build_summary_uses_last_stdout_line_on_success() {
        let stdout = "line1\nfinal line\n";
        let summary = build_summary(stdout, "", 0);
        assert_eq!(summary, "final line");
    }

    #[test]
    fn test_build_summary_uses_stdout_on_failure_when_present() {
        let stdout = "details\nreason from stdout\n";
        let summary = build_summary(stdout, "stderr message", 2);
        assert_eq!(summary, "reason from stdout");
    }

    #[test]
    fn test_build_summary_falls_back_to_stderr_on_failure() {
        let summary = build_summary("\n", "stderr reason\n", 3);
        assert_eq!(summary, "stderr reason");
    }

    #[test]
    fn test_build_summary_falls_back_to_exit_code_when_no_output() {
        let summary = build_summary("", "   \n", -1);
        assert_eq!(summary, "exit code -1");
    }

    // ── ForkMethod / ForkInfo / fork routing tests ──────────────────

    #[test]
    fn test_fork_method_for_tool_claude_code_is_native() {
        assert_eq!(
            TransportFactory::fork_method_for_tool("claude-code"),
            ForkMethod::Native
        );
    }

    #[test]
    #[cfg(feature = "codex-pty-fork")]
    fn test_fork_method_for_tool_codex_is_native_when_feature_enabled() {
        assert_eq!(
            TransportFactory::fork_method_for_tool("codex"),
            ForkMethod::Native
        );
    }

    #[test]
    #[cfg(not(feature = "codex-pty-fork"))]
    fn test_fork_method_for_tool_codex_is_soft_when_feature_disabled() {
        assert_eq!(
            TransportFactory::fork_method_for_tool("codex"),
            ForkMethod::Soft
        );
    }

    #[test]
    fn test_fork_method_for_tool_gemini_cli_is_soft() {
        assert_eq!(
            TransportFactory::fork_method_for_tool("gemini-cli"),
            ForkMethod::Soft
        );
    }

    #[test]
    fn test_fork_method_for_tool_opencode_is_soft() {
        assert_eq!(
            TransportFactory::fork_method_for_tool("opencode"),
            ForkMethod::Soft
        );
    }

    #[test]
    fn test_fork_info_construction_success() {
        let info = ForkInfo {
            success: true,
            method: ForkMethod::Native,
            new_session_id: Some("new-sess-123".to_string()),
            notes: None,
        };
        assert!(info.success);
        assert_eq!(info.method, ForkMethod::Native);
        assert_eq!(info.new_session_id.as_deref(), Some("new-sess-123"));
        assert!(info.notes.is_none());
    }

    #[test]
    fn test_fork_info_construction_failure() {
        let info = ForkInfo {
            success: false,
            method: ForkMethod::Soft,
            new_session_id: None,
            notes: Some("something went wrong".to_string()),
        };
        assert!(!info.success);
        assert_eq!(info.method, ForkMethod::Soft);
        assert!(info.new_session_id.is_none());
        assert!(info.notes.as_deref().unwrap().contains("wrong"));
    }

    #[tokio::test]
    async fn test_fork_session_soft_with_empty_parent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let request = ForkRequest {
            tool_name: "codex".to_string(),
            fork_method: None,
            provider_session_id: None,
            codex_auto_trust: false,
            parent_csa_session_id: "01TEST_PARENT".to_string(),
            parent_session_dir: tmp.path().to_path_buf(),
            working_dir: tmp.path().to_path_buf(),
            timeout: Duration::from_secs(10),
        };

        let info = TransportFactory::fork_session(&request).await;

        assert!(
            info.success,
            "Soft fork should succeed even with empty parent: {:?}",
            info.notes
        );
        assert_eq!(info.method, ForkMethod::Soft);
        assert!(
            info.new_session_id.is_none(),
            "Soft fork does not create provider session"
        );
        assert!(
            info.notes
                .as_deref()
                .unwrap()
                .contains("ready for injection")
        );
    }

    #[tokio::test]
    async fn test_fork_session_codex_native_falls_back_to_soft_when_provider_id_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let request = ForkRequest {
            tool_name: "codex".to_string(),
            fork_method: Some(ForkMethod::Native),
            provider_session_id: None,
            codex_auto_trust: false,
            parent_csa_session_id: "01TEST_PARENT".to_string(),
            parent_session_dir: tmp.path().to_path_buf(),
            working_dir: tmp.path().to_path_buf(),
            timeout: Duration::from_secs(10),
        };

        let info = TransportFactory::fork_session(&request).await;

        assert!(info.success);
        assert_eq!(info.method, ForkMethod::Soft);
        assert!(info.new_session_id.is_none());
        assert!(
            info.notes
                .as_deref()
                .unwrap_or_default()
                .contains("Native codex fork degraded")
        );
    }

    #[tokio::test]
    async fn test_fork_session_native_without_provider_id_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let request = ForkRequest {
            tool_name: "claude-code".to_string(),
            fork_method: None,
            provider_session_id: None,
            codex_auto_trust: false,
            parent_csa_session_id: "01TEST_PARENT".to_string(),
            parent_session_dir: tmp.path().to_path_buf(),
            working_dir: tmp.path().to_path_buf(),
            timeout: Duration::from_secs(10),
        };

        let info = TransportFactory::fork_session(&request).await;

        assert!(!info.success);
        assert_eq!(info.method, ForkMethod::Native);
        assert!(
            info.notes
                .as_deref()
                .unwrap()
                .contains("provider_session_id"),
            "Should explain missing provider ID: {:?}",
            info.notes
        );
    }

    #[tokio::test]
    async fn test_fork_session_soft_with_result_toml() {
        use chrono::Utc;
        use csa_session::result::{RESULT_FILE_NAME, SessionResult};

        let tmp = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let result = SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "Tests passed".to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: vec![],
        };
        std::fs::write(
            tmp.path().join(RESULT_FILE_NAME),
            toml::to_string_pretty(&result).unwrap(),
        )
        .unwrap();

        let request = ForkRequest {
            tool_name: "gemini-cli".to_string(),
            fork_method: None,
            provider_session_id: None,
            codex_auto_trust: false,
            parent_csa_session_id: "01RICH_PARENT".to_string(),
            parent_session_dir: tmp.path().to_path_buf(),
            working_dir: tmp.path().to_path_buf(),
            timeout: Duration::from_secs(10),
        };

        let info = TransportFactory::fork_session(&request).await;

        assert!(info.success);
        assert_eq!(info.method, ForkMethod::Soft);
        assert!(
            info.notes
                .as_deref()
                .unwrap()
                .contains("ready for injection")
        );
    }

    include!("transport_tests_tail.rs");
}

#[cfg(test)]
#[path = "transport_lean_mode_tests.rs"]
mod lean_mode_tests;
