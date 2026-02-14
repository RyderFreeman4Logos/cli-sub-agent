use std::collections::HashMap;
use std::path::Path;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use csa_acp::{SessionConfig, SessionEvent};
use csa_process::{ExecutionResult, StreamMode, spawn_tool, wait_and_capture};
use csa_session::state::{MetaSessionState, ToolState};

use crate::executor::Executor;

const SUMMARY_MAX_CHARS: usize = 200;

/// Transport abstraction for executing prompts via different protocols.
/// Implementations: LegacyTransport (CLI non-interactive) and AcpTransport (ACP protocol).
#[async_trait]
pub trait Transport: Send + Sync {
    /// Execute a prompt and return the result.
    async fn execute(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        stream_mode: StreamMode,
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
    ) -> Result<TransportResult> {
        let (cmd, stdin_data) = self
            .executor
            .build_execute_in_command(prompt, work_dir, extra_env);
        let child = spawn_tool(cmd, stdin_data).await?;
        let execution = wait_and_capture(child, stream_mode).await?;

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
        stream_mode: StreamMode,
    ) -> Result<TransportResult> {
        let (cmd, stdin_data) = self
            .executor
            .build_command(prompt, tool_state, session, extra_env);
        let child = spawn_tool(cmd, stdin_data).await?;
        let execution = wait_and_capture(child, stream_mode).await?;

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
        match tool_name {
            "claude-code" => ("claude".into(), vec!["--acp".into()]),
            "codex" => ("codex".into(), vec!["acp".into()]),
            "opencode" => ("opencode".into(), vec!["acp".into()]),
            "gemini-cli" => ("gemini".into(), vec!["--experimental-acp".into()]),
            _ => (tool_name.into(), vec!["--acp".into()]),
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

    fn build_env(
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
    async fn execute(
        &self,
        prompt: &str,
        _tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        stream_mode: StreamMode,
    ) -> Result<TransportResult> {
        if stream_mode != StreamMode::BufferOnly {
            tracing::debug!(
                "ACP transport does not yet support stream_mode={:?}; output will be buffered",
                stream_mode
            );
        }

        let env = self.build_env(session, extra_env);
        let working_dir = Path::new(&session.project_path).to_path_buf();
        let system_prompt = Self::build_system_prompt(self.session_config.as_ref());
        let acp_command = self.acp_command.clone();
        let acp_args = self.acp_args.clone();
        let prompt = prompt.to_string();

        // csa-acp currently relies on !Send internals (LocalSet/Rc). Run it on a
        // dedicated current-thread runtime so callers can stay Send-safe.
        let output =
            tokio::task::spawn_blocking(move || -> Result<csa_acp::transport::AcpOutput> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| anyhow!("failed to build ACP runtime: {e}"))?;
                rt.block_on(csa_acp::transport::run_prompt(
                    &acp_command,
                    &acp_args,
                    &working_dir,
                    &env,
                    system_prompt.as_deref(),
                    &prompt,
                ))
                .map_err(|e| anyhow!("ACP transport failed: {e}"))
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
            ("claude".to_string(), vec!["--acp".to_string()])
        );
        assert_eq!(
            AcpTransport::acp_command_for_tool("codex"),
            ("codex".to_string(), vec!["acp".to_string()])
        );
        assert_eq!(
            AcpTransport::acp_command_for_tool("opencode"),
            ("opencode".to_string(), vec!["acp".to_string()])
        );
        assert_eq!(
            AcpTransport::acp_command_for_tool("gemini-cli"),
            ("gemini".to_string(), vec!["--experimental-acp".to_string()])
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
}
