//! Executor enum for 6 AI tools.

use anyhow::Result;
use csa_core::types::{PromptTransport, ToolName};
use csa_process::ExecutionResult;
use csa_session::state::{MetaSessionState, ToolState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::process::Command;

use crate::claude_runtime::{
    ClaudeCodeRuntimeMetadata, ClaudeCodeTransport, claude_runtime_metadata,
};
use crate::codex_runtime::{CodexRuntimeMetadata, CodexTransport, codex_runtime_metadata};
#[cfg(feature = "acp")]
use crate::hermes_config::HermesRunConfig;
use crate::install_hints::{
    ANTIGRAVITY_CLI_INSTALL_HINT, GEMINI_CLI_INSTALL_HINT, HERMES_INSTALL_HINT,
    OPENAI_COMPAT_INSTALL_HINT, OPENCODE_INSTALL_HINT,
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

#[path = "executor_antigravity_settings.rs"]
pub(crate) mod antigravity_settings;
pub(crate) use antigravity_settings::AntigravitySettingsGuard;

#[path = "executor_codex_tmux.rs"]
mod codex_tmux;
#[path = "executor_command.rs"]
mod command;
#[path = "executor_env.rs"]
pub(crate) mod executor_env;
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
    OpenaiCompat {
        model_override: Option<String>,
        thinking_budget: Option<ThinkingBudget>,
    },
    Hermes {
        provider_override: Option<String>,
        model_override: Option<String>,
        thinking_budget: Option<ThinkingBudget>,
    },
    AntigravityCli {
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
    pub fn tool_name(&self) -> &'static str {
        match self {
            Self::GeminiCli { .. } => "gemini-cli",
            Self::Opencode { .. } => "opencode",
            Self::Codex { .. } => "codex",
            Self::ClaudeCode { .. } => "claude-code",
            Self::OpenaiCompat { .. } => "openai-compat",
            Self::Hermes { .. } => "hermes",
            Self::AntigravityCli { .. } => "antigravity-cli",
        }
    }

    pub fn executable_name(&self) -> &'static str {
        match self {
            Self::GeminiCli { .. } => "gemini",
            Self::Opencode { .. } => "opencode",
            Self::Codex { .. } => "codex",
            Self::ClaudeCode { .. } => "claude",
            Self::OpenaiCompat { .. } => "openai-compat",
            Self::Hermes { .. } => "hermes",
            Self::AntigravityCli { .. } => "antigravity",
        }
    }

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
            Self::OpenaiCompat { .. } => "openai-compat",
            Self::Hermes { .. } => "hermes",
            Self::AntigravityCli { .. } => "antigravity",
        }
    }

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
            Self::Hermes { .. } => HERMES_INSTALL_HINT,
            Self::AntigravityCli { .. } => ANTIGRAVITY_CLI_INSTALL_HINT,
        }
    }

    pub fn yolo_args(&self) -> &[&str] {
        match self {
            Self::GeminiCli { .. } | Self::AntigravityCli { .. } => &["-y"],
            Self::Opencode { .. } => &[] as &[&str],
            Self::Codex { .. } => &["--dangerously-bypass-approvals-and-sandbox"],
            Self::ClaudeCode { .. } => &["--dangerously-skip-permissions"],
            Self::OpenaiCompat { .. } | Self::Hermes { .. } => &[] as &[&str],
        }
    }

    /// Construct executor from model spec.
    pub fn from_spec(spec: &ModelSpec) -> Result<Self> {
        let budget = Some(spec.thinking_budget.clone());
        let tool = match spec.tool.as_str() {
            "gemini-cli" => ToolName::GeminiCli,
            "opencode" => ToolName::Opencode,
            "codex" => ToolName::Codex,
            "claude-code" => ToolName::ClaudeCode,
            "openai-compat" => ToolName::OpenaiCompat,
            "hermes" => ToolName::Hermes,
            "antigravity-cli" => ToolName::AntigravityCli,
            other => anyhow::bail!("Unknown tool '{other}' in model spec"),
        };
        let model = Some(if matches!(tool, ToolName::Opencode) {
            format!("{}/{}", spec.provider, spec.model)
        } else {
            spec.model.clone()
        });
        if matches!(tool, ToolName::Hermes) {
            return Ok(Self::Hermes {
                provider_override: Some(spec.provider.clone()),
                model_override: model,
                thinking_budget: budget,
            });
        }
        Ok(Self::from_tool_name(&tool, model, budget))
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
            ToolName::Hermes => Self::Hermes {
                provider_override: None,
                model_override: model,
                thinking_budget,
            },
            ToolName::AntigravityCli => Self::AntigravityCli {
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
            }
            | Self::Hermes {
                thinking_budget, ..
            }
            | Self::AntigravityCli {
                thinking_budget, ..
            } => thinking_budget.as_ref(),
        }
    }

    /// Returns the model identity that will be handed to the selected tool.
    pub fn model_override(&self) -> Option<&str> {
        match self {
            Self::GeminiCli { model_override, .. }
            | Self::Opencode { model_override, .. }
            | Self::Codex { model_override, .. }
            | Self::ClaudeCode { model_override, .. }
            | Self::OpenaiCompat { model_override, .. }
            | Self::Hermes { model_override, .. }
            | Self::AntigravityCli { model_override, .. } => model_override.as_deref(),
        }
    }

    /// Returns an executor-level provider override when the transport carries
    /// one independently from the model string.
    pub fn provider_override(&self) -> Option<&str> {
        match self {
            Self::Hermes {
                provider_override, ..
            } => provider_override.as_deref(),
            _ => None,
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
            }
            | Self::Hermes {
                thinking_budget, ..
            }
            | Self::AntigravityCli {
                thinking_budget, ..
            } => {
                *thinking_budget = Some(budget);
            }
        }
    }

    /// Stage the antigravity-cli model override in `settings.json` (if any).
    ///
    /// For [`Executor::AntigravityCli`] this returns an
    /// [`AntigravitySettingsGuard`] that rewrites
    /// `~/.gemini/antigravity-cli/settings.json` to point at the configured
    /// model and restores it when dropped. For every other executor variant
    /// this is a no-op returning `Ok(None)`.
    ///
    /// The returned guard MUST be kept alive for the duration of the spawned
    /// process so that `agy` reads the staged model at startup.
    pub(crate) fn antigravity_settings_guard(&self) -> Result<Option<AntigravitySettingsGuard>> {
        match self {
            Self::AntigravityCli { model_override, .. } => {
                AntigravitySettingsGuard::apply_model(model_override)
            }
            _ => Ok(None),
        }
    }

    /// Override the model (CLI `--model` / config `[review].model` > tier model_spec).
    pub fn override_model(&mut self, model: String) {
        match self {
            Self::GeminiCli { model_override, .. }
            | Self::Opencode { model_override, .. }
            | Self::Codex { model_override, .. }
            | Self::ClaudeCode { model_override, .. }
            | Self::OpenaiCompat { model_override, .. }
            | Self::Hermes { model_override, .. }
            | Self::AntigravityCli { model_override, .. } => {
                *model_override = Some(model);
            }
        }
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
            error_marker_scan_enabled: options.error_marker_scan_enabled,
            setting_sources: options.setting_sources.clone(),
            sandbox: sandbox_transport.as_ref(),
            thinking_budget: self.thinking_budget().cloned(),
            subtree_pin: options.subtree_pin.clone(),
            allow_git_push: options.allow_git_push,
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

    /// Execute in a specific directory (ephemeral sessions, `extra_env` for API keys etc.).
    ///
    /// `subtree_pin` carries CSA's authoritative subtree model pin (#1741),
    /// out-of-band from `extra_env`; it is the only channel that may set the
    /// pin keys on the child. Pass `None` when CSA did not decide to pin.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
        allow_git_push: bool,
        stream_mode: csa_process::StreamMode,
        idle_timeout_seconds: u64,
        initial_response_timeout: ResolvedTimeout,
    ) -> Result<ExecutionResult> {
        Ok(self
            .execute_in_with_transport(
                prompt,
                work_dir,
                extra_env,
                subtree_pin,
                allow_git_push,
                stream_mode,
                idle_timeout_seconds,
                initial_response_timeout,
            )
            .await?
            .execution)
    }

    /// Execute in a specific directory and keep transport metadata.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_in_with_transport(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
        allow_git_push: bool,
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
                subtree_pin,
                allow_git_push,
                stream_mode,
                idle_timeout_seconds,
                initial_response_timeout,
            )
            .await?;
        result.execution.consolidate_stderr_retries();
        Ok(result)
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
        csa_core::env::scrub_subtree_contract_env_tokio(&mut cmd);

        self.inject_csa_owned_env(&mut cmd, session);

        cmd
    }

    fn inject_csa_owned_env(&self, cmd: &mut Command, session: &MetaSessionState) {
        cmd.env("CSA_SESSION_ID", &session.meta_session_id);
        cmd.env("CSA_DEPTH", (session.genealogy.depth + 1).to_string());
        cmd.env("CSA_PROJECT_ROOT", &session.project_path);
        cmd.env(csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY, "1");
        Self::inject_session_path_env(cmd, session);

        cmd.env("CSA_TOOL", self.tool_name());
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
                    csa_session::next_turn_contract_result_path(&dir, session.turn_count)
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

    #[cfg(feature = "acp")]
    pub(crate) fn hermes_run_config(&self) -> Option<HermesRunConfig> {
        match self {
            Self::Hermes {
                provider_override,
                model_override,
                thinking_budget,
            } => {
                let (provider, model) = hermes_dispatch_identity(
                    provider_override.as_deref(),
                    model_override.as_deref(),
                );
                Some(HermesRunConfig::new(
                    provider.map(str::to_string),
                    model.map(str::to_string),
                    thinking_budget.clone(),
                ))
            }
            _ => None,
        }
    }
}

include!("executor_tool_args.rs");
include!("executor_runtime_transport.rs");

#[cfg(test)]
#[path = "executor_build_cmd_tests.rs"]
mod build_cmd_tests;
#[cfg(test)]
#[path = "executor_prompt_transport_tests.rs"]
mod prompt_transport_tests;
#[cfg(test)]
#[path = "executor_tests.rs"]
mod tests;
