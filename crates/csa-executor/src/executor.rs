//! Executor enum for 4 AI tools.

use anyhow::{Result, bail};
use csa_core::types::{PromptTransport, ToolName};
use csa_process::ExecutionResult;
use csa_session::state::{MetaSessionState, ToolState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use tokio::process::Command;

use crate::claude_runtime::{
    ClaudeCodeRuntimeMetadata, ClaudeCodeTransport, claude_runtime_metadata,
};
use crate::codex_runtime::{CodexRuntimeMetadata, CodexTransport, codex_runtime_metadata};
use crate::install_hints::{
    GEMINI_CLI_INSTALL_HINT, OPENAI_COMPAT_INSTALL_HINT, OPENCODE_INSTALL_HINT,
};
use crate::lefthook_guard::{sanitize_args_for_codex, sanitize_env_for_codex};
use crate::model_spec::{ModelSpec, ThinkingBudget};
use crate::session_config::SessionConfig;
use crate::transport::{
    ResolvedTimeout, SandboxTransportConfig, Transport, TransportFactory, TransportOptions,
    TransportResult,
};
#[path = "executor_arg_helpers.rs"]
mod arg_helpers;
use arg_helpers::{
    append_gemini_include_directories_args, codex_notify_suppression_args,
    effective_gemini_model_override, gemini_include_directories,
};

#[path = "executor_env.rs"]
mod executor_env;
#[path = "executor_pre_session.rs"]
mod pre_session;
#[path = "executor_prompt_helpers.rs"]
mod prompt_helpers;
#[path = "executor_restrictions.rs"]
mod restrictions;

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
        #[serde(default = "default_claude_runtime_metadata")]
        runtime_metadata: ClaudeCodeRuntimeMetadata,
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

const fn default_claude_runtime_metadata() -> ClaudeCodeRuntimeMetadata {
    ClaudeCodeRuntimeMetadata::current()
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
            Self::ClaudeCode {
                runtime_metadata, ..
            } => runtime_metadata.runtime_binary_name(),
            Self::OpenaiCompat { .. } => "openai-compat", // no binary; HTTP-only
        }
    }

    /// Get installation instructions for the tool.
    pub fn install_hint(&self) -> &'static str {
        match self {
            Self::GeminiCli { .. } => GEMINI_CLI_INSTALL_HINT,
            Self::Opencode { .. } => OPENCODE_INSTALL_HINT,
            Self::Codex {
                runtime_metadata, ..
            } => runtime_metadata.install_hint(),
            Self::ClaudeCode {
                runtime_metadata, ..
            } => runtime_metadata.install_hint(),
            Self::OpenaiCompat { .. } => OPENAI_COMPAT_INSTALL_HINT,
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
                runtime_metadata: claude_runtime_metadata(),
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
                runtime_metadata: claude_runtime_metadata(),
            },
            ToolName::OpenaiCompat => Self::OpenaiCompat {
                model_override: model,
                thinking_budget,
            },
        }
    }

    /// Returns the current thinking budget, if any.
    pub fn thinking_budget(&self) -> Option<&ThinkingBudget> {
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
            } => thinking_budget.as_ref(),
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

    /// Inject environment variables from global config into a Command.
    pub fn inject_env(cmd: &mut Command, env_vars: &HashMap<String, String>) {
        for (key, value) in env_vars {
            if !Self::STRIPPED_ENV_VARS.contains(&key.as_str()) {
                cmd.env(key, value);
            }
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
        self.inject_csa_owned_env(&mut cmd, session);
        executor_env::inject_git_guard_env(&mut cmd);
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
        if matches!(self, Self::Codex { .. }) {
            sanitize_env_for_codex(&mut cmd);
            cmd = Self::sanitize_codex_command_args(cmd);
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
            isolation_plan: ctx.isolation_plan.clone(),
            tool_name: ctx.tool_name.clone(),
            session_id: ctx.session_id.clone(),
            best_effort: ctx.best_effort,
        });
        let transport_options = TransportOptions {
            stream_mode: options.stream_mode,
            idle_timeout_seconds: options.idle_timeout_seconds,
            acp_crash_max_attempts: options.acp_crash_max_attempts,
            initial_response_timeout: ResolvedTimeout(options.initial_response_timeout_seconds),
            liveness_dead_seconds: options.liveness_dead_seconds,
            stdin_write_timeout_seconds: options.stdin_write_timeout_seconds,
            acp_init_timeout_seconds: options.acp_init_timeout_seconds,
            termination_grace_period_seconds: options.termination_grace_period_seconds,
            output_spool: options.output_spool.as_deref(),
            output_spool_max_bytes: options.output_spool_max_bytes,
            output_spool_keep_rotated: options.output_spool_keep_rotated,
            setting_sources: options.setting_sources.clone(),
            sandbox: sandbox_transport.as_ref(),
            thinking_budget: self.thinking_budget().cloned(),
        };
        let transport = self.transport(session_config)?;
        let effective_prompt = self.apply_pre_session_hook(prompt, session, &options).await;
        let mut result = transport
            .execute(
                &effective_prompt,
                tool_state,
                session,
                extra_env,
                transport_options,
            )
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
        initial_response_timeout: ResolvedTimeout,
    ) -> Result<ExecutionResult> {
        Ok(self
            .execute_in_with_transport(
                prompt,
                work_dir,
                extra_env,
                stream_mode,
                idle_timeout_seconds,
                initial_response_timeout,
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
        initial_response_timeout: ResolvedTimeout,
    ) -> Result<TransportResult> {
        let transport = self.transport(None)?;
        let mut result = transport
            .execute_in(
                prompt,
                work_dir,
                extra_env,
                stream_mode,
                idle_timeout_seconds,
                initial_response_timeout,
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
        executor_env::inject_git_guard_env(&mut cmd);
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
        if matches!(self, Self::Codex { .. }) {
            sanitize_env_for_codex(&mut cmd);
            cmd = Self::sanitize_codex_command_args(cmd);
        }
        (cmd, stdin_data)
    }

    fn sanitize_codex_command_args(cmd: Command) -> Command {
        let program = cmd.as_std().get_program().to_os_string();
        let current_dir = cmd.as_std().get_current_dir().map(|dir| dir.to_path_buf());
        let envs = cmd
            .as_std()
            .get_envs()
            .map(|(key, value)| (key.to_os_string(), value.map(|v| v.to_os_string())))
            .collect::<Vec<_>>();
        let mut args = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        sanitize_args_for_codex(&mut args);

        let mut sanitized = Command::new(program);
        if let Some(dir) = current_dir {
            sanitized.current_dir(dir);
        }
        for (key, value) in envs {
            match value {
                Some(value) => {
                    sanitized.env(key, value);
                }
                None => {
                    sanitized.env_remove(key);
                }
            }
        }
        let os_args = args.into_iter().map(OsString::from).collect::<Vec<_>>();
        sanitized.args(os_args);
        sanitized
    }

    /// Environment variables to strip from child processes.
    const STRIPPED_ENV_VARS: &[&str] = executor_env::STRIPPED_ENV_VARS;

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

        self.inject_csa_owned_env(&mut cmd, session);

        cmd
    }

    fn inject_csa_owned_env(&self, cmd: &mut Command, session: &MetaSessionState) {
        cmd.env("CSA_SESSION_ID", &session.meta_session_id);
        cmd.env("CSA_DEPTH", (session.genealogy.depth + 1).to_string());
        cmd.env("CSA_PROJECT_ROOT", &session.project_path);
        Self::inject_session_path_env(cmd, session);

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
        let codex_resume = matches!(self, Self::Codex { .. })
            && tool_state
                .and_then(|state| state.provider_session_id.as_deref())
                .is_some();

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
                cmd.arg("--json");
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
                    cmd.arg("resume").arg(session_id);
                }
                Self::ClaudeCode { .. }
                    if matches!(self.claude_code_transport(), Some(ClaudeCodeTransport::Acp)) =>
                {
                    cmd.arg("--resume").arg(session_id);
                }
                Self::OpenaiCompat { .. } => {} // HTTP-only
                Self::ClaudeCode { .. } => {}
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
                    Self::Codex { .. } if codex_resume => {
                        cmd.arg("-");
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
                        ThinkingBudget::Max => "max",
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
                ..
            } => {
                if let Some(model) = model_override {
                    cmd.arg("--model").arg(model);
                }
                // claude-code 2.x exposes thinking control via `--effort
                // <level>` (low/medium/high/xhigh/max); the legacy
                // `--thinking-budget <tokens>` flag was removed and any
                // emission of it makes the binary exit with `unknown option`
                // (#1124). `DefaultBudget` deliberately omits the flag so the
                // tool applies its built-in default.
                if let Some(budget) = thinking_budget
                    && let Some(level) = budget.claude_effort()
                {
                    cmd.arg("--effort").arg(level);
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
}

include!("executor_runtime_transport.rs");

#[cfg(test)]
#[path = "executor_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "executor_build_cmd_tests.rs"]
mod build_cmd_tests;

#[cfg(test)]
#[path = "executor_prompt_transport_tests.rs"]
mod prompt_transport_tests;
