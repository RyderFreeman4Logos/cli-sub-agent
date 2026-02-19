//! Executor enum for 4 AI tools.

use anyhow::{Result, bail};
use csa_acp::SessionConfig;
use csa_core::types::{PromptTransport, ToolName, prompt_transport_capabilities};
use csa_process::{ExecutionResult, StreamMode};
use csa_session::state::{MetaSessionState, ToolState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::model_spec::{ModelSpec, ThinkingBudget};
use crate::transport::{
    LegacyTransport, SandboxTransportConfig, Transport, TransportFactory, TransportOptions,
    TransportResult,
};

pub const MAX_ARGV_PROMPT_LEN: usize = 100 * 1024;

/// Options for tool execution, including stream mode, timeouts, and optional sandbox config.
#[derive(Debug, Clone)]
pub struct ExecuteOptions {
    pub stream_mode: StreamMode,
    pub idle_timeout_seconds: u64,
    pub output_spool: Option<PathBuf>,
    /// Optional resource sandbox config (cgroup/rlimit limits).
    /// When `Some`, the spawned tool process will be wrapped in resource isolation.
    pub sandbox: Option<SandboxContext>,
}

/// Sandbox configuration resolved from project/tool config.
///
/// Carries the resource limits together with identifiers needed to name
/// the cgroup scope (tool name + session ID).
#[derive(Debug, Clone)]
pub struct SandboxContext {
    /// Resource limits to apply.
    pub config: csa_resource::cgroup::SandboxConfig,
    /// Tool name for scope naming (e.g. "claude-code").
    pub tool_name: String,
    /// Session ID for scope naming.
    pub session_id: String,
    /// When true, sandbox spawn failures fall back to unsandboxed spawn.
    pub best_effort: bool,
}

impl ExecuteOptions {
    pub fn new(stream_mode: StreamMode, idle_timeout_seconds: u64) -> Self {
        Self {
            stream_mode,
            idle_timeout_seconds,
            output_spool: None,
            sandbox: None,
        }
    }

    /// Set sandbox context for resource isolation.
    pub fn with_sandbox(mut self, sandbox: SandboxContext) -> Self {
        self.sandbox = Some(sandbox);
        self
    }

    /// Set output spool file path for incremental/final output persistence.
    pub fn with_output_spool(mut self, output_spool: PathBuf) -> Self {
        self.output_spool = Some(output_spool);
        self
    }
}

/// Executor: Closed enum for 4 AI tools.
///
/// Uses data enum pattern (not trait + dynamic dispatch) for a fixed set of tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "kebab-case")]
pub enum Executor {
    GeminiCli {
        model_override: Option<String>,
        thinking_budget: Option<ThinkingBudget>,
    },
    Opencode {
        model_override: Option<String>,
        agent: Option<String>,
        thinking_budget: Option<ThinkingBudget>,
    },
    Codex {
        model_override: Option<String>,
        thinking_budget: Option<ThinkingBudget>,
    },
    ClaudeCode {
        model_override: Option<String>,
        thinking_budget: Option<ThinkingBudget>,
    },
}

impl Executor {
    /// Get the tool name as a string.
    pub fn tool_name(&self) -> &'static str {
        match self {
            Self::GeminiCli { .. } => "gemini-cli",
            Self::Opencode { .. } => "opencode",
            Self::Codex { .. } => "codex",
            Self::ClaudeCode { .. } => "claude-code",
        }
    }

    /// Get the executable name for legacy CLI transport.
    ///
    /// Used only by `LegacyTransport` to build the CLI command.
    /// For pre-flight availability checks, use `runtime_binary_name()` instead.
    pub fn executable_name(&self) -> &'static str {
        match self {
            Self::GeminiCli { .. } => "gemini",
            Self::Opencode { .. } => "opencode",
            Self::Codex { .. } => "codex",
            Self::ClaudeCode { .. } => "claude",
        }
    }

    /// Get the binary name that will actually be spawned at runtime.
    ///
    /// ACP-routed tools use standalone adapter binaries (`codex-acp`,
    /// `claude-code-acp`). Legacy tools use the native CLI binary.
    /// Use this for pre-flight `check_tool_installed` calls.
    pub fn runtime_binary_name(&self) -> &'static str {
        match self {
            Self::GeminiCli { .. } => "gemini",
            Self::Opencode { .. } => "opencode",
            Self::Codex { .. } => "codex-acp",
            Self::ClaudeCode { .. } => "claude-code-acp",
        }
    }

    /// Get installation instructions for the tool.
    pub fn install_hint(&self) -> &'static str {
        match self {
            Self::GeminiCli { .. } => "Install: npm install -g @anthropic-ai/gemini-cli",
            Self::Opencode { .. } => "Install: go install github.com/anthropics/opencode@latest",
            Self::Codex { .. } => "Install ACP adapter: npm install -g @zed-industries/codex-acp",
            Self::ClaudeCode { .. } => {
                "Install ACP adapter: npm install -g @zed-industries/claude-code-acp"
            }
        }
    }

    /// Get "yolo" args for the tool (bypass approval prompts).
    pub fn yolo_args(&self) -> &[&str] {
        match self {
            Self::GeminiCli { .. } => &["-y"],
            Self::Opencode { .. } => &[] as &[&str], // opencode does not have a yolo mode
            Self::Codex { .. } => &["--dangerously-bypass-approvals-and-sandbox"],
            Self::ClaudeCode { .. } => &["--dangerously-skip-permissions"],
        }
    }

    /// Construct executor from model spec.
    pub fn from_spec(spec: &ModelSpec) -> Result<Self> {
        let model = Some(spec.model.clone());
        let budget = Some(spec.thinking_budget.clone());
        match spec.tool.as_str() {
            "gemini-cli" => Ok(Self::GeminiCli {
                model_override: model,
                thinking_budget: budget,
            }),
            "opencode" => Ok(Self::Opencode {
                model_override: model,
                agent: None,
                thinking_budget: budget,
            }),
            "codex" => Ok(Self::Codex {
                model_override: model,
                thinking_budget: budget,
            }),
            "claude-code" => Ok(Self::ClaudeCode {
                model_override: model,
                thinking_budget: budget,
            }),
            other => bail!("Unknown tool '{}' in model spec", other),
        }
    }

    /// Construct executor from ToolName enum with optional model and thinking budget.
    pub fn from_tool_name(
        tool: &ToolName,
        model: Option<String>,
        thinking_budget: Option<ThinkingBudget>,
    ) -> Self {
        match tool {
            ToolName::GeminiCli => Self::GeminiCli {
                model_override: model,
                thinking_budget,
            },
            ToolName::Opencode => Self::Opencode {
                model_override: model,
                agent: None,
                thinking_budget,
            },
            ToolName::Codex => Self::Codex {
                model_override: model,
                thinking_budget,
            },
            ToolName::ClaudeCode => Self::ClaudeCode {
                model_override: model,
                thinking_budget,
            },
        }
    }

    /// Apply restrictions by modifying the prompt to include restriction instructions.
    /// Returns the modified prompt.
    ///
    /// When `allow_edit` is false, injects a restriction message that prevents
    /// the tool from modifying existing files.
    pub fn apply_restrictions(&self, prompt: &str, allow_edit: bool) -> String {
        if !allow_edit {
            format!(
                "IMPORTANT RESTRICTION: You MUST NOT edit or modify any existing files. \
                 You may only create new files or perform read-only analysis.\n\n{}",
                prompt
            )
        } else {
            prompt.to_string()
        }
    }

    /// Inject environment variables from global config into a Command.
    pub fn inject_env(cmd: &mut Command, env_vars: &HashMap<String, String>) {
        for (key, value) in env_vars {
            cmd.env(key, value);
        }
    }

    /// Build a configured Command ready for execution.
    ///
    /// Returns the Command object without executing it, allowing caller to:
    /// - Spawn the process and get its PID
    /// - Start resource monitoring
    /// - Wait for completion separately
    ///
    /// `extra_env`: optional environment variables to inject (e.g., API keys from GlobalConfig).
    pub fn build_command(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
    ) -> (Command, Option<Vec<u8>>) {
        let mut cmd = self.build_base_command(session);
        if let Some(env) = extra_env {
            Self::inject_env(&mut cmd, env);
        }
        let (prompt_transport, stdin_data) = self.select_prompt_transport(prompt);
        if matches!(prompt_transport, PromptTransport::Argv) {
            self.append_tool_args(&mut cmd, prompt, tool_state);
        } else {
            self.append_tool_args_with_transport(&mut cmd, prompt, tool_state, prompt_transport);
        }
        (cmd, stdin_data)
    }

    /// Execute a task with full session context.
    pub async fn execute(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        stream_mode: csa_process::StreamMode,
        idle_timeout_seconds: u64,
    ) -> Result<ExecutionResult> {
        Ok(self
            .execute_with_transport(
                prompt,
                tool_state,
                session,
                extra_env,
                ExecuteOptions::new(stream_mode, idle_timeout_seconds),
                None,
            )
            .await?
            .execution)
    }

    /// Execute and keep transport metadata (provider session ID, event stream).
    #[tracing::instrument(skip_all, fields(tool = %self.tool_name()))]
    pub async fn execute_with_transport(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        options: ExecuteOptions,
        session_config: Option<SessionConfig>,
    ) -> Result<TransportResult> {
        let sandbox_transport = options.sandbox.as_ref().map(|ctx| SandboxTransportConfig {
            config: ctx.config.clone(),
            tool_name: ctx.tool_name.clone(),
            session_id: ctx.session_id.clone(),
            best_effort: ctx.best_effort,
        });
        let transport_options = TransportOptions {
            stream_mode: options.stream_mode,
            idle_timeout_seconds: options.idle_timeout_seconds,
            output_spool: options.output_spool.as_deref(),
            sandbox: sandbox_transport.as_ref(),
        };
        let transport = self.transport(session_config);
        transport
            .execute(prompt, tool_state, session, extra_env, transport_options)
            .await
    }

    /// Execute in a specific directory (for ephemeral sessions).
    ///
    /// `extra_env`: optional environment variables to inject (e.g., API keys from GlobalConfig).
    pub async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        stream_mode: csa_process::StreamMode,
        idle_timeout_seconds: u64,
    ) -> Result<ExecutionResult> {
        Ok(self
            .execute_in_with_transport(
                prompt,
                work_dir,
                extra_env,
                stream_mode,
                idle_timeout_seconds,
            )
            .await?
            .execution)
    }

    /// Execute in a specific directory and keep transport metadata.
    pub async fn execute_in_with_transport(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        stream_mode: csa_process::StreamMode,
        idle_timeout_seconds: u64,
    ) -> Result<TransportResult> {
        let legacy = LegacyTransport::new(self.clone());
        legacy
            .execute_in(
                prompt,
                work_dir,
                extra_env,
                stream_mode,
                idle_timeout_seconds,
            )
            .await
    }

    /// Build command for execute_in() legacy path.
    pub(crate) fn build_execute_in_command(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
    ) -> (Command, Option<Vec<u8>>) {
        let mut cmd = Command::new(self.executable_name());
        cmd.current_dir(work_dir);
        // Strip recursive-invocation guard vars (same as build_base_command).
        for var in Self::STRIPPED_ENV_VARS {
            cmd.env_remove(var);
        }
        if let Some(env) = extra_env {
            Self::inject_env(&mut cmd, env);
        }
        self.append_yolo_args(&mut cmd);
        self.append_model_args(&mut cmd);
        let (prompt_transport, stdin_data) = self.select_prompt_transport(prompt);
        if matches!(prompt_transport, PromptTransport::Argv) {
            self.append_prompt_args(&mut cmd, prompt);
        } else {
            self.append_prompt_args_with_transport(&mut cmd, prompt, prompt_transport);
        }
        (cmd, stdin_data)
    }

    /// Environment variables to strip from child processes.
    ///
    /// These prevent recursive-invocation guards in CLI tools from blocking
    /// legitimate CSA sub-agent launches.  Mirrors the same list in
    /// `csa-acp::AcpConnection::STRIPPED_ENV_VARS`.
    const STRIPPED_ENV_VARS: &[&str] = &[
        // Claude Code sets this to detect recursive invocations.  When
        // inherited by a child process, the child refuses to start.
        "CLAUDECODE",
        // Entrypoint tracking for the parent session — not meaningful for
        // a fresh sub-agent invocation.
        "CLAUDE_CODE_ENTRYPOINT",
    ];

    /// Build base command with session environment variables.
    fn build_base_command(&self, session: &MetaSessionState) -> Command {
        let mut cmd = Command::new(self.executable_name());
        cmd.current_dir(&session.project_path);

        // Strip environment variables that would trigger recursive-invocation
        // guards in child tool processes (e.g., Claude Code's CLAUDECODE check).
        for var in Self::STRIPPED_ENV_VARS {
            cmd.env_remove(var);
        }

        // Set CSA environment variables
        cmd.env("CSA_SESSION_ID", &session.meta_session_id);
        cmd.env("CSA_DEPTH", (session.genealogy.depth + 1).to_string());
        cmd.env("CSA_PROJECT_ROOT", &session.project_path);

        // CSA_TOOL: tells the child process which tool it is running as
        cmd.env("CSA_TOOL", self.tool_name());
        // CSA_PARENT_TOOL: tells the child process which tool its parent is
        // (read from current process's CSA_TOOL, set by the parent CSA instance)
        if let Ok(current_tool) = std::env::var("CSA_TOOL") {
            cmd.env("CSA_PARENT_TOOL", current_tool);
        }

        if let Some(ref parent) = session.genealogy.parent_session_id {
            cmd.env("CSA_PARENT_SESSION", parent);
        }

        cmd
    }

    fn transport(&self, session_config: Option<SessionConfig>) -> Box<dyn Transport> {
        TransportFactory::create(self, session_config)
    }

    /// Append tool-specific arguments for full execution.
    ///
    /// Delegates to `append_yolo_args`, `append_model_args`, `append_prompt_args`,
    /// and adds session-resume and tool-specific structural args.
    fn append_tool_args(&self, cmd: &mut Command, prompt: &str, tool_state: Option<&ToolState>) {
        self.append_tool_args_with_transport(cmd, prompt, tool_state, PromptTransport::Argv);
    }

    fn append_tool_args_with_transport(
        &self,
        cmd: &mut Command,
        prompt: &str,
        tool_state: Option<&ToolState>,
        prompt_transport: PromptTransport,
    ) {
        // Structural args (subcommand, output format, yolo) come first
        match self {
            Self::GeminiCli { .. } => {
                // gemini: -p prompt -m model -y [-r session]
            }
            Self::Opencode { .. } => {
                cmd.arg("run");
                cmd.arg("--format").arg("json");
            }
            Self::Codex { .. } => {
                cmd.arg("exec");
                cmd.arg("--dangerously-bypass-approvals-and-sandbox");
            }
            Self::ClaudeCode { .. } => {
                cmd.arg("--dangerously-skip-permissions");
                cmd.arg("--output-format").arg("json");
            }
        }

        // Model and thinking budget (shared with execute_in)
        self.append_model_args(cmd);

        // Yolo flag for gemini (other tools handle it in structural args above)
        if matches!(self, Self::GeminiCli { .. }) {
            cmd.arg("-y");
        }

        // Session resume
        if let Some(state) = tool_state {
            if let Some(ref session_id) = state.provider_session_id {
                match self {
                    Self::GeminiCli { .. } => {
                        cmd.arg("-r").arg(session_id);
                    }
                    Self::Opencode { .. } => {
                        cmd.arg("-s").arg(session_id);
                    }
                    Self::Codex { .. } => {
                        cmd.arg("--session-id").arg(session_id);
                    }
                    Self::ClaudeCode { .. } => {
                        cmd.arg("--resume").arg(session_id);
                    }
                }
            }
        }

        // Prompt (position matters per tool)
        match prompt_transport {
            PromptTransport::Argv => match self {
                Self::GeminiCli { .. } | Self::ClaudeCode { .. } => {
                    cmd.arg("-p").arg(prompt);
                }
                Self::Opencode { .. } | Self::Codex { .. } => {
                    cmd.arg(prompt);
                }
            },
            PromptTransport::Stdin => {
                // When prompt is delivered via stdin, tools that use `-p` for
                // non-interactive/pipe mode still need the flag (without the
                // prompt argument) so they read from stdin instead of entering
                // interactive mode.
                match self {
                    Self::GeminiCli { .. } | Self::ClaudeCode { .. } => {
                        cmd.arg("-p");
                    }
                    Self::Opencode { .. } | Self::Codex { .. } => {
                        // These tools read from stdin natively without extra flags.
                    }
                }
            }
        }
    }

    /// Append model override and thinking budget args (tool-specific flags).
    fn append_model_args(&self, cmd: &mut Command) {
        match self {
            Self::GeminiCli {
                model_override,
                thinking_budget,
            } => {
                if let Some(model) = model_override {
                    cmd.arg("-m").arg(model);
                }
                if let Some(budget) = thinking_budget {
                    cmd.arg("--thinking_budget")
                        .arg(budget.token_count().to_string());
                }
            }
            Self::Opencode {
                model_override,
                agent,
                thinking_budget,
            } => {
                if let Some(model) = model_override {
                    cmd.arg("-m").arg(model);
                }
                if let Some(agent_name) = agent {
                    cmd.arg("--agent").arg(agent_name);
                }
                if let Some(budget) = thinking_budget {
                    let variant = match budget {
                        ThinkingBudget::DefaultBudget => "medium",
                        ThinkingBudget::Low => "minimal",
                        ThinkingBudget::Medium => "medium",
                        ThinkingBudget::High => "high",
                        ThinkingBudget::Xhigh => "max",
                        ThinkingBudget::Custom(_) => "max",
                    };
                    cmd.arg("--variant").arg(variant);
                }
            }
            Self::Codex {
                model_override,
                thinking_budget,
                ..
            } => {
                if let Some(model) = model_override {
                    cmd.arg("--model").arg(model);
                }
                if let Some(budget) = thinking_budget {
                    cmd.arg("-c")
                        .arg(format!("model_reasoning_effort={}", budget.codex_effort()));
                }
            }
            Self::ClaudeCode {
                model_override,
                thinking_budget,
            } => {
                if let Some(model) = model_override {
                    cmd.arg("--model").arg(model);
                }
                if let Some(budget) = thinking_budget {
                    cmd.arg("--thinking-budget")
                        .arg(budget.token_count().to_string());
                }
            }
        }
    }

    /// Append "yolo" args (bypass approvals).
    fn append_yolo_args(&self, cmd: &mut Command) {
        for arg in self.yolo_args() {
            cmd.arg(arg);
        }
    }

    /// Append minimal prompt args for execute_in.
    fn append_prompt_args(&self, cmd: &mut Command, prompt: &str) {
        self.append_prompt_args_with_transport(cmd, prompt, PromptTransport::Argv);
    }

    fn append_prompt_args_with_transport(
        &self,
        cmd: &mut Command,
        prompt: &str,
        prompt_transport: PromptTransport,
    ) {
        match self {
            Self::GeminiCli { .. } => {
                if matches!(prompt_transport, PromptTransport::Argv) {
                    cmd.arg("-p").arg(prompt);
                }
            }
            Self::Opencode { .. } => {
                cmd.arg("run");
                if matches!(prompt_transport, PromptTransport::Argv) {
                    cmd.arg(prompt);
                }
            }
            Self::Codex { .. } => {
                cmd.arg("exec");
                if matches!(prompt_transport, PromptTransport::Argv) {
                    cmd.arg(prompt);
                }
            }
            Self::ClaudeCode { .. } => {
                if matches!(prompt_transport, PromptTransport::Argv) {
                    cmd.arg("-p").arg(prompt);
                }
            }
        }
    }

    fn select_prompt_transport(&self, prompt: &str) -> (PromptTransport, Option<Vec<u8>>) {
        if prompt.len() <= MAX_ARGV_PROMPT_LEN {
            return (PromptTransport::Argv, None);
        }

        let tool = self.tool_name_enum();
        let supports_stdin = prompt_transport_capabilities(&tool).contains(&PromptTransport::Stdin);
        if supports_stdin {
            return (PromptTransport::Stdin, Some(prompt.as_bytes().to_vec()));
        }

        tracing::warn!(
            tool = self.tool_name(),
            prompt_len = prompt.len(),
            max_argv_prompt_len = MAX_ARGV_PROMPT_LEN,
            "Prompt exceeds argv threshold; tool supports argv-only transport"
        );
        (PromptTransport::Argv, None)
    }

    fn tool_name_enum(&self) -> ToolName {
        match self {
            Self::GeminiCli { .. } => ToolName::GeminiCli,
            Self::Opencode { .. } => ToolName::Opencode,
            Self::Codex { .. } => ToolName::Codex,
            Self::ClaudeCode { .. } => ToolName::ClaudeCode,
        }
    }
}

#[cfg(test)]
#[path = "executor_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "executor_build_cmd_tests.rs"]
mod build_cmd_tests;

#[cfg(test)]
#[path = "executor_prompt_transport_tests.rs"]
mod prompt_transport_tests;
