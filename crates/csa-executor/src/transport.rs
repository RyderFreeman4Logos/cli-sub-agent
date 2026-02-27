use std::collections::HashMap;
use std::path::Path;

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

#[path = "transport_fork.rs"]
mod transport_fork;
pub use transport_fork::{ForkInfo, ForkMethod, ForkRequest};

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

    include!("transport_tests_tail.rs");
}

#[cfg(test)]
#[path = "transport_lean_mode_tests.rs"]
mod lean_mode_tests;
