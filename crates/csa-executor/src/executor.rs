//! Executor enum for 4 AI tools.

use anyhow::{Result, bail};
use csa_acp::SessionConfig;
use csa_core::types::{PromptTransport, ToolName, prompt_transport_capabilities};
use csa_process::ExecutionResult;
use csa_session::state::{MetaSessionState, ToolState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::process::Command;

use crate::codex_runtime::{CodexRuntimeMetadata, CodexTransport, codex_runtime_metadata};
use crate::model_spec::{ModelSpec, ThinkingBudget};
use crate::transport::{
    SandboxTransportConfig, Transport, TransportFactory, TransportOptions, TransportResult,
};
#[path = "executor_arg_helpers.rs"]
mod arg_helpers;
use arg_helpers::{
    append_gemini_include_directories_args, codex_notify_suppression_args,
    effective_gemini_model_override, gemini_include_directories,
};

pub const MAX_ARGV_PROMPT_LEN: usize = 100 * 1024;

#[path = "executor_options.rs"]
mod options;
pub use options::{ExecuteOptions, SandboxContext};

/// Executor: Closed enum for AI tools.
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
        #[serde(default = "default_codex_runtime_metadata")]
        runtime_metadata: CodexRuntimeMetadata,
    },
    ClaudeCode {
        model_override: Option<String>,
        thinking_budget: Option<ThinkingBudget>,
    },
    /// OpenAI-compatible HTTP API tool (no CLI process, pure HTTP).
    OpenaiCompat {
        model_override: Option<String>,
        thinking_budget: Option<ThinkingBudget>,
    },
}

const fn default_codex_runtime_metadata() -> CodexRuntimeMetadata {
    CodexRuntimeMetadata::current()
}

impl Executor {
    /// Get the tool name as a string.
    pub fn tool_name(&self) -> &'static str {
        match self {
            Self::GeminiCli { .. } => "gemini-cli",
            Self::Opencode { .. } => "opencode",
            Self::Codex { .. } => "codex",
            Self::ClaudeCode { .. } => "claude-code",
            Self::OpenaiCompat { .. } => "openai-compat",
        }
    }

    /// Executable name for `LegacyTransport` CLI commands.
    /// For availability checks, use `runtime_binary_name()`.
    pub fn executable_name(&self) -> &'static str {
        match self {
            Self::GeminiCli { .. } => "gemini",
            Self::Opencode { .. } => "opencode",
            Self::Codex { .. } => "codex",
            Self::ClaudeCode { .. } => "claude",
            Self::OpenaiCompat { .. } => "openai-compat", // no CLI binary
        }
    }

    /// Binary spawned at runtime (ACP adapters or native CLI).
    pub fn runtime_binary_name(&self) -> &'static str {
        match self {
            Self::GeminiCli { .. } => "gemini",
            Self::Opencode { .. } => "opencode",
            Self::Codex {
                runtime_metadata, ..
            } => runtime_metadata.runtime_binary_name(),
            Self::ClaudeCode { .. } => "claude-code-acp",
            Self::OpenaiCompat { .. } => "openai-compat", // no binary; HTTP-only
        }
    }

    /// Get installation instructions for the tool.
    pub fn install_hint(&self) -> &'static str {
        match self {
            Self::GeminiCli { .. } => "Install: npm install -g @anthropic-ai/gemini-cli",
            Self::Opencode { .. } => "Install: go install github.com/anthropics/opencode@latest",
            Self::Codex {
                runtime_metadata, ..
            } => runtime_metadata.install_hint(),
            Self::ClaudeCode { .. } => {
                "Install ACP adapter: npm install -g @zed-industries/claude-code-acp"
            }
            Self::OpenaiCompat { .. } => {
                "Configure [tools.openai-compat] with base_url and api_key in config.toml"
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
            Self::OpenaiCompat { .. } => &[] as &[&str], // HTTP-only, no CLI args
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
                runtime_metadata: codex_runtime_metadata(),
            }),
            "claude-code" => Ok(Self::ClaudeCode {
                model_override: model,
                thinking_budget: budget,
            }),
            "openai-compat" => Ok(Self::OpenaiCompat {
                model_override: model,
                thinking_budget: budget,
            }),
            other => bail!("Unknown tool '{other}' in model spec"),
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
                runtime_metadata: codex_runtime_metadata(),
            },
            ToolName::ClaudeCode => Self::ClaudeCode {
                model_override: model,
                thinking_budget,
            },
            ToolName::OpenaiCompat => Self::OpenaiCompat {
                model_override: model,
                thinking_budget,
            },
        }
    }

    /// Override the thinking budget (thinking_lock replaces whatever was set).
    pub fn override_thinking_budget(&mut self, budget: ThinkingBudget) {
        match self {
            Self::GeminiCli {
                thinking_budget, ..
            }
            | Self::Opencode {
                thinking_budget, ..
            }
            | Self::Codex {
                thinking_budget, ..
            }
            | Self::ClaudeCode {
                thinking_budget, ..
            }
            | Self::OpenaiCompat {
                thinking_budget, ..
            } => {
                *thinking_budget = Some(budget);
            }
        }
    }

    /// Override the model (CLI `--model` / config `[review].model` > tier model_spec).
    pub fn override_model(&mut self, model: String) {
        match self {
            Self::GeminiCli { model_override, .. }
            | Self::Opencode { model_override, .. }
            | Self::Codex { model_override, .. }
            | Self::ClaudeCode { model_override, .. }
            | Self::OpenaiCompat { model_override, .. } => {
                *model_override = Some(model);
            }
        }
    }

    /// Override codex runtime transport metadata.
    pub fn override_codex_transport(&mut self, transport: CodexTransport) {
        if let Self::Codex {
            runtime_metadata, ..
        } = self
        {
            *runtime_metadata = CodexRuntimeMetadata::from_transport(transport);
        }
    }

    #[must_use]
    pub fn codex_transport(&self) -> Option<CodexTransport> {
        match self {
            Self::Codex {
                runtime_metadata, ..
            } => Some(runtime_metadata.transport_mode()),
            _ => None,
        }
    }

    /// Apply restrictions by modifying the prompt to include restriction instructions.
    /// Returns the modified prompt.
    ///
    /// `allow_edit`: when false, tool must not modify existing files.
    /// `allow_write_new`: when false, tool must not create new files either.
    pub fn apply_restrictions(
        &self,
        prompt: &str,
        allow_edit: bool,
        allow_write_new: bool,
    ) -> String {
        if !allow_edit && !allow_write_new {
            format!(
                "IMPORTANT RESTRICTION: You are in READ-ONLY mode. \
                 You MUST NOT edit existing files, create new files, or run commands \
                 that modify the filesystem. You may ONLY perform read-only analysis, \
                 search, and reporting.\n\n{prompt}"
            )
        } else if !allow_edit {
            format!(
                "IMPORTANT RESTRICTION: You MUST NOT edit or modify any existing files. \
                 You may only create new files or perform read-only analysis.\n\n{prompt}"
            )
        } else if !allow_write_new {
            format!(
                "IMPORTANT RESTRICTION: You MUST NOT create new files. \
                 You may only edit existing files or perform read-only analysis.\n\n{prompt}"
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

    /// Build a configured Command ready for execution (without spawning).
    pub fn build_command(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
    ) -> (Command, Option<Vec<u8>>) {
        let mut cmd = self.build_base_command(session);
        if matches!(self, Self::GeminiCli { .. }) {
            Self::strip_gemini_inherited_env(&mut cmd);
        }
        if let Some(env) = extra_env {
            Self::inject_env(&mut cmd, env);
        }
        Self::inject_session_path_env(&mut cmd, session);
        let gemini_include_directories =
            gemini_include_directories(extra_env, prompt, Some(Path::new(&session.project_path)));
        let (prompt_transport, stdin_data) = self.select_prompt_transport(prompt);
        self.append_tool_args_with_transport(
            &mut cmd,
            prompt,
            tool_state,
            prompt_transport,
            &gemini_include_directories,
        );
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
            isolation_plan: ctx.isolation_plan.clone(),
            tool_name: ctx.tool_name.clone(),
            session_id: ctx.session_id.clone(),
            best_effort: ctx.best_effort,
        });
        let transport_options = TransportOptions {
            stream_mode: options.stream_mode,
            idle_timeout_seconds: options.idle_timeout_seconds,
            acp_crash_max_attempts: options.acp_crash_max_attempts,
            initial_response_timeout_seconds: options.initial_response_timeout_seconds,
            liveness_dead_seconds: options.liveness_dead_seconds,
            stdin_write_timeout_seconds: options.stdin_write_timeout_seconds,
            acp_init_timeout_seconds: options.acp_init_timeout_seconds,
            termination_grace_period_seconds: options.termination_grace_period_seconds,
            output_spool: options.output_spool.as_deref(),
            output_spool_max_bytes: options.output_spool_max_bytes,
            output_spool_keep_rotated: options.output_spool_keep_rotated,
            setting_sources: options.setting_sources.clone(),
            sandbox: sandbox_transport.as_ref(),
        };
        let transport = self.transport(session_config)?;
        let mut result = transport
            .execute(prompt, tool_state, session, extra_env, transport_options)
            .await?;
        result.execution.consolidate_stderr_retries();
        Ok(result)
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
        let transport = self.transport(None)?;
        let mut result = transport
            .execute_in(
                prompt,
                work_dir,
                extra_env,
                stream_mode,
                idle_timeout_seconds,
            )
            .await?;
        result.execution.consolidate_stderr_retries();
        Ok(result)
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
        if matches!(self, Self::GeminiCli { .. }) {
            Self::strip_gemini_inherited_env(&mut cmd);
        }
        if let Some(env) = extra_env {
            Self::inject_env(&mut cmd, env);
        }
        let gemini_include_directories =
            gemini_include_directories(extra_env, prompt, Some(work_dir));
        self.append_yolo_args(&mut cmd);
        self.append_model_args(&mut cmd);
        if matches!(self, Self::GeminiCli { .. }) {
            append_gemini_include_directories_args(&mut cmd, &gemini_include_directories);
        }
        if matches!(self, Self::Codex { .. })
            && let Some(env) = extra_env
        {
            cmd.args(codex_notify_suppression_args(env));
        }
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
    /// legitimate CSA sub-agent launches, and ensure hook-bypass env vars
    /// never leak into child tool processes.  Mirrors the same list in
    /// `csa-acp::AcpConnection::STRIPPED_ENV_VARS`.
    const STRIPPED_ENV_VARS: &[&str] = &[
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "LEFTHOOK",
        "LEFTHOOK_SKIP",
    ];

    /// Strip process-inherited gemini auth/routing env vars so that CSA's
    /// extra_env controls auth mode exclusively (OAuth-first, API key fallback
    /// only after quota exhaustion).  Without this, a process-level
    /// `GEMINI_API_KEY` bypasses the entire OAuth→model-switch→API-key
    /// degradation chain.
    fn strip_gemini_inherited_env(cmd: &mut Command) {
        for var in csa_core::gemini::INHERITED_ENV_STRIP {
            cmd.env_remove(var);
        }
    }

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
        Self::inject_session_path_env(&mut cmd, session);

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

    fn inject_session_path_env(cmd: &mut Command, session: &MetaSessionState) {
        match csa_session::manager::get_session_dir(
            Path::new(&session.project_path),
            &session.meta_session_id,
        ) {
            Ok(dir) => {
                cmd.env("CSA_SESSION_DIR", dir.to_string_lossy().into_owned());
                cmd.env(
                    csa_session::RESULT_TOML_PATH_CONTRACT_ENV,
                    csa_session::contract_result_path(&dir)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            Err(e) => {
                tracing::warn!("failed to compute CSA_SESSION_DIR: {e:#}");
            }
        }
    }

    pub(crate) fn transport(
        &self,
        session_config: Option<SessionConfig>,
    ) -> Result<Box<dyn Transport>> {
        TransportFactory::create(self, session_config)
    }

    /// Append tool-specific arguments for full execution.
    ///
    /// Delegates to `append_yolo_args`, `append_model_args`, `append_prompt_args`,
    /// and adds session-resume and tool-specific structural args.
    #[cfg(test)]
    fn append_tool_args(&self, cmd: &mut Command, prompt: &str, tool_state: Option<&ToolState>) {
        self.append_tool_args_with_transport(cmd, prompt, tool_state, PromptTransport::Argv, &[]);
    }

    fn append_tool_args_with_transport(
        &self,
        cmd: &mut Command,
        prompt: &str,
        tool_state: Option<&ToolState>,
        prompt_transport: PromptTransport,
        gemini_include_directories: &[String],
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
            Self::OpenaiCompat { .. } => {} // HTTP-only
        }

        // Model and thinking budget (shared with execute_in)
        self.append_model_args(cmd);

        // Yolo flag for gemini (other tools handle it in structural args above)
        if matches!(self, Self::GeminiCli { .. }) {
            cmd.arg("-y");
            append_gemini_include_directories_args(cmd, gemini_include_directories);
        }

        // Session resume
        if let Some(state) = tool_state
            && let Some(ref session_id) = state.provider_session_id
        {
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
                Self::OpenaiCompat { .. } => {} // HTTP-only
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
                Self::OpenaiCompat { .. } => {} // HTTP-only
            },
            PromptTransport::Stdin => {
                match self {
                    Self::GeminiCli { .. } | Self::ClaudeCode { .. } => {
                        cmd.arg("-p");
                    }
                    Self::Opencode { .. } | Self::Codex { .. } | Self::OpenaiCompat { .. } => {
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
                if let Some(model) = effective_gemini_model_override(model_override) {
                    cmd.arg("-m").arg(model);
                }
                if thinking_budget.is_some() {
                    // gemini-cli (0.31+) no longer accepts thinking-budget flags.
                    // Ignore CSA thinking hints and let gemini-cli decide routing.
                    tracing::debug!(
                        "Ignoring thinking budget for gemini-cli because runtime no longer supports a thinking flag"
                    );
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
            Self::OpenaiCompat { .. } => {} // HTTP-only: model/thinking via API body
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
            Self::OpenaiCompat { .. } => {} // HTTP-only
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
            Self::OpenaiCompat { .. } => ToolName::OpenaiCompat,
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
