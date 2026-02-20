use std::collections::HashMap;
use std::path::Path;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use csa_acp::{SessionConfig, SessionEvent};
use csa_process::{
    ExecutionResult, StreamMode, spawn_tool, spawn_tool_sandboxed,
    wait_and_capture_with_idle_timeout,
};
use csa_resource::cgroup::SandboxConfig;
use csa_session::state::{MetaSessionState, ToolState};

use crate::executor::Executor;

const SUMMARY_MAX_CHARS: usize = 200;

/// Sandbox configuration passed through the transport layer.
///
/// Carries cgroup/rlimit limits together with identifiers needed for
/// scope naming.  This is the transport-layer counterpart of
/// [`crate::executor::SandboxContext`].
#[derive(Debug, Clone)]
pub struct SandboxTransportConfig {
    /// Resource limits to apply (memory, swap, PIDs).
    pub config: SandboxConfig,
    /// Tool name for cgroup scope naming (e.g. "claude-code").
    pub tool_name: String,
    /// Session ID for cgroup scope naming.
    /// When true, sandbox spawn failures fall back to unsandboxed spawn.
    pub best_effort: bool,
    pub session_id: String,
}

/// Bundled execution options passed through the transport layer.
///
/// Groups stream mode, idle timeout, and optional sandbox config into a single
/// parameter to keep the `Transport::execute` signature within clippy's argument limit.
#[derive(Debug, Clone)]
pub struct TransportOptions<'a> {
    pub stream_mode: StreamMode,
    pub idle_timeout_seconds: u64,
    pub output_spool: Option<&'a Path>,
    pub lean_mode: bool,
    pub sandbox: Option<&'a SandboxTransportConfig>,
}

/// Transport abstraction for executing prompts via different protocols.
/// Implementations: LegacyTransport (CLI non-interactive) and AcpTransport (ACP protocol).
#[async_trait]
pub trait Transport: Send + Sync {
    /// Execute a prompt and return the result.
    ///
    /// When `options.sandbox` is provided, the spawned tool process will be wrapped
    /// in resource isolation (cgroup scope or setrlimit fallback).
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

/// Result from transport execution, extending ExecutionResult with transport-specific data.
#[derive(Debug, Clone)]
pub struct TransportResult {
    pub execution: ExecutionResult,
    /// Provider session ID (if protocol transport provided one directly).
    pub provider_session_id: Option<String>,
    /// ACP session events for audit (if ACP transport was used).
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
        let child = spawn_tool(cmd, stdin_data).await?;
        let execution = wait_and_capture_with_idle_timeout(
            child,
            stream_mode,
            std::time::Duration::from_secs(idle_timeout_seconds),
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
                let child = spawn_tool(
                    self.executor
                        .build_command(prompt, tool_state, session, extra_env)
                        .0,
                    stdin_data,
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
    tool_name: String,
    /// ACP command to spawn (e.g., "claude", "codex")
    acp_command: String,
    /// ACP command args
    acp_args: Vec<String>,
    /// Session config from .skill.toml
    session_config: Option<SessionConfig>,
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

    fn build_system_prompt(session_config: Option<&SessionConfig>) -> Option<String> {
        let config = session_config?;
        let mut sections = Vec::new();

        if !config.no_load.is_empty() {
            sections.push(format!("No-load skills: {}", config.no_load.join(", ")));
        }
        if !config.extra_load.is_empty() {
            sections.push(format!(
                "Extra-load skills: {}",
                config.extra_load.join(", ")
            ));
        }
        if let Some(tier) = &config.tier {
            sections.push(format!("Tier: {tier}"));
        }
        if !config.models.is_empty() {
            sections.push(format!("Model candidates: {}", config.models.join(", ")));
        }
        if !config.mcp_servers.is_empty() {
            let servers = config
                .mcp_servers
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            sections.push(format!("MCP servers: {servers}"));
        }

        if sections.is_empty() {
            None
        } else {
            Some(sections.join("\n"))
        }
    }

    pub(crate) fn build_lean_mode_meta(
        lean_mode: bool,
    ) -> Option<serde_json::Map<String, serde_json::Value>> {
        if !lean_mode {
            return None;
        }
        let serde_json::Value::Object(meta) =
            serde_json::json!({"claudeCode": {"options": {"settingSources": []}}})
        else {
            return None;
        };
        Some(meta)
    }

    pub(crate) fn build_env(
        &self,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
    ) -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert(
            "CSA_SESSION_ID".to_string(),
            session.meta_session_id.clone(),
        );
        env.insert(
            "CSA_DEPTH".to_string(),
            (session.genealogy.depth + 1).to_string(),
        );
        env.insert("CSA_PROJECT_ROOT".to_string(), session.project_path.clone());
        // CSA_SESSION_DIR: absolute path to the session state directory
        if let Ok(dir) = csa_session::manager::get_session_dir(
            Path::new(&session.project_path),
            &session.meta_session_id,
        ) {
            env.insert(
                "CSA_SESSION_DIR".to_string(),
                dir.to_string_lossy().into_owned(),
            );
        } else {
            tracing::warn!("failed to compute CSA_SESSION_DIR for ACP env");
        }

        env.insert("CSA_TOOL".to_string(), self.tool_name.clone());
        if let Ok(parent_tool) = std::env::var("CSA_TOOL") {
            env.insert("CSA_PARENT_TOOL".to_string(), parent_tool);
        }
        if let Some(parent_session) = &session.genealogy.parent_session_id {
            env.insert("CSA_PARENT_SESSION".to_string(), parent_session.clone());
        }

        if let Some(extra) = extra_env {
            env.extend(extra.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        env
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
        let session_meta = Self::build_lean_mode_meta(options.lean_mode);
        let stream_stdout_to_stderr = options.stream_mode != StreamMode::BufferOnly;
        let output_spool = options.output_spool.map(std::path::Path::to_path_buf);

        // csa-acp currently relies on !Send internals (LocalSet/Rc). Run it on a
        // dedicated current-thread runtime so callers can stay Send-safe.
        let output =
            tokio::task::spawn_blocking(move || -> Result<csa_acp::transport::AcpOutput> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| anyhow!("failed to build ACP runtime: {e}"))?;

                if let Some(ref cfg) = sandbox_config {
                    // Sandboxed path: use AcpConnection::spawn_sandboxed directly,
                    // then replicate the session setup from run_prompt.
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
                                },
                                &prompt,
                                csa_acp::transport::AcpRunOptions {
                                    idle_timeout: std::time::Duration::from_secs(
                                        idle_timeout_seconds,
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
                        },
                        &prompt,
                        csa_acp::transport::AcpRunOptions {
                            idle_timeout: std::time::Duration::from_secs(idle_timeout_seconds),
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

/// Run an ACP prompt with sandbox isolation.
#[allow(clippy::too_many_arguments)]
async fn run_acp_sandboxed(
    command: &str,
    args: &[String],
    working_dir: &Path,
    env: &HashMap<String, String>,
    system_prompt: Option<&str>,
    resume_session_id: Option<&str>,
    meta: Option<serde_json::Map<String, serde_json::Value>>,
    prompt: &str,
    idle_timeout: std::time::Duration,
    sandbox_config: &SandboxConfig,
    tool_name: &str,
    session_id: &str,
    stream_stdout_to_stderr: bool,
    output_spool: Option<&Path>,
) -> csa_acp::AcpResult<csa_acp::transport::AcpOutput> {
    use csa_acp::AcpConnection;

    let (connection, _sandbox_handle) = AcpConnection::spawn_sandboxed(
        command,
        args,
        working_dir,
        env,
        Some(sandbox_config),
        tool_name,
        session_id,
    )
    .await?;

    connection.initialize().await?;

    let acp_session_id = if let Some(resume_id) = resume_session_id {
        tracing::debug!(
            resume_session_id = resume_id,
            "loading ACP session (sandboxed)"
        );
        match connection.load_session(resume_id, Some(working_dir)).await {
            Ok(id) => id,
            Err(error) => {
                tracing::warn!(
                    resume_session_id = resume_id,
                    error = %error,
                    "Failed to resume sandboxed ACP session, creating new session"
                );
                connection
                    .new_session(system_prompt, Some(working_dir), meta.clone())
                    .await?
            }
        }
    } else {
        connection
            .new_session(system_prompt, Some(working_dir), meta.clone())
            .await?
    };

    let result = connection
        .prompt_with_io(
            &acp_session_id,
            prompt,
            idle_timeout,
            csa_acp::connection::PromptIoOptions {
                stream_stdout_to_stderr,
                output_spool,
            },
        )
        .await?;

    let mut exit_code = connection.exit_code().await?.unwrap_or(0);
    let mut stderr = connection.stderr();
    if result.timed_out {
        exit_code = 137;
        if !stderr.is_empty() && !stderr.ends_with('\n') {
            stderr.push('\n');
        }
        stderr.push_str(&format!(
            "idle timeout: no ACP events/stderr for {}s; process killed",
            idle_timeout.as_secs()
        ));
        stderr.push('\n');
    }

    // _sandbox_handle dropped here, cleaning up cgroup scope if applicable.

    Ok(csa_acp::transport::AcpOutput {
        output: result.output,
        stderr,
        events: result.events,
        session_id: acp_session_id,
        exit_code,
    })
}

fn build_summary(stdout: &str, stderr: &str, exit_code: i32) -> String {
    if exit_code == 0 {
        return truncate_line(last_non_empty_line(stdout), SUMMARY_MAX_CHARS);
    }

    let stdout_line = last_non_empty_line(stdout);
    if !stdout_line.is_empty() {
        return truncate_line(stdout_line, SUMMARY_MAX_CHARS);
    }

    let stderr_line = last_non_empty_line(stderr);
    if !stderr_line.is_empty() {
        return truncate_line(stderr_line, SUMMARY_MAX_CHARS);
    }

    format!("exit code {exit_code}")
}

fn last_non_empty_line(output: &str) -> &str {
    output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
}

fn truncate_line(line: &str, max_chars: usize) -> String {
    line.chars().take(max_chars).collect()
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

    // NOTE: CSA_SUPPRESS_NOTIFY is injected by the pipeline layer (not transport)
    // based on per-tool config via extra_env. See pipeline.rs suppress_notify logic.
    #[test]
    fn test_acp_build_env_propagates_extra_env() {
        let transport = AcpTransport::new("claude-code", None);
        let now = chrono::Utc::now();
        let session = csa_session::state::MetaSessionState {
            meta_session_id: "01HTEST000000000000000000".to_string(),
            description: Some("test".to_string()),
            project_path: "/tmp/test".to_string(),
            created_at: now,
            last_accessed: now,
            genealogy: csa_session::state::Genealogy {
                parent_session_id: None,
                depth: 0,
            },
            tools: HashMap::new(),
            context_status: csa_session::state::ContextStatus::default(),
            total_token_usage: None,
            phase: csa_session::state::SessionPhase::Active,
            task_context: csa_session::state::TaskContext::default(),
            turn_count: 0,
            token_budget: None,
            sandbox_info: None,
        };

        let mut extra = HashMap::new();
        extra.insert("CSA_SUPPRESS_NOTIFY".to_string(), "1".to_string());
        let env = transport.build_env(&session, Some(&extra));
        assert_eq!(
            env.get("CSA_SUPPRESS_NOTIFY"),
            Some(&"1".to_string()),
            "ACP transport should propagate CSA_SUPPRESS_NOTIFY from extra_env"
        );

        // Without extra_env, suppress_notify should NOT be present.
        let env_no_extra = transport.build_env(&session, None);
        assert_eq!(
            env_no_extra.get("CSA_SUPPRESS_NOTIFY"),
            None,
            "ACP transport should not inject CSA_SUPPRESS_NOTIFY on its own"
        );
    }

    #[test]
    fn test_acp_build_env_includes_csa_session_dir() {
        let transport = AcpTransport::new("claude-code", None);
        let now = chrono::Utc::now();
        let session = csa_session::state::MetaSessionState {
            meta_session_id: "01HTEST000000000000000000".to_string(),
            description: Some("test".to_string()),
            project_path: "/tmp/test".to_string(),
            created_at: now,
            last_accessed: now,
            genealogy: csa_session::state::Genealogy {
                parent_session_id: None,
                depth: 0,
            },
            tools: HashMap::new(),
            context_status: csa_session::state::ContextStatus::default(),
            total_token_usage: None,
            phase: csa_session::state::SessionPhase::Active,
            task_context: csa_session::state::TaskContext::default(),
            turn_count: 0,
            token_budget: None,
            sandbox_info: None,
        };

        let env = transport.build_env(&session, None);
        let session_dir = env
            .get("CSA_SESSION_DIR")
            .expect("CSA_SESSION_DIR should be present in env");
        assert!(
            session_dir.contains("/sessions/"),
            "CSA_SESSION_DIR should contain /sessions/ path segment, got: {session_dir}"
        );
        assert!(
            session_dir.contains("01HTEST000000000000000000"),
            "CSA_SESSION_DIR should contain the session ID, got: {session_dir}"
        );
    }

    #[test]
    fn test_resume_session_id_extraction() {
        let now = chrono::Utc::now();
        let tool_state = ToolState {
            provider_session_id: Some("test-session-123".to_string()),
            last_action_summary: String::new(),
            last_exit_code: 0,
            updated_at: now,
            token_usage: None,
        };
        let resume_id = tool_state.provider_session_id.as_deref();
        assert_eq!(resume_id, Some("test-session-123"));
    }

    #[test]
    fn test_resume_session_id_none_when_absent() {
        let now = chrono::Utc::now();
        let tool_state = ToolState {
            provider_session_id: None,
            last_action_summary: String::new(),
            last_exit_code: 0,
            updated_at: now,
            token_usage: None,
        };
        let resume_id = tool_state.provider_session_id.as_deref();
        assert!(resume_id.is_none());
    }
}

#[cfg(test)]
#[path = "transport_lean_mode_tests.rs"]
mod lean_mode_tests;
