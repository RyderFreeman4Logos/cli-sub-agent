// Tool-argv composition for [`Executor`].
//
// Split out of `executor.rs` (#1620) to keep that file under the per-module
// token budget. Each method is a [`Command`] mutator that contributes the
// tool-specific flags / positional args for a given [`Executor`] variant.
// This file is `include!`d into `executor.rs`, so the impl block below
// continues the same `impl Executor` namespace.

fn hermes_dispatch_identity<'a>(
    provider_override: Option<&'a str>,
    model_override: Option<&'a str>,
) -> (Option<&'a str>, Option<&'a str>) {
    match model_override.and_then(|model| model.split_once('/')) {
        Some((provider, model)) => (Some(provider), Some(model)),
        None => (provider_override, model_override),
    }
}

impl Executor {
    /// Append tool-specific arguments for full execution.
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
            Self::Hermes { .. } => {
                cmd.arg("run");
            }
            Self::OpenaiCompat { .. } | Self::AntigravityCli { .. } => {}
        }

        // Model and thinking budget (shared with execute_in)
        self.append_model_args(cmd);

        // Yolo flag for gemini/antigravity (other tools handle it in structural args above)
        if matches!(self, Self::GeminiCli { .. } | Self::AntigravityCli { .. }) {
            cmd.arg("-y");
            append_gemini_include_directories_args(cmd, gemini_include_directories);
        }

        // Session resume
        if let Some(state) = tool_state
            && let Some(ref session_id) = state.provider_session_id
        {
            match self {
                Self::GeminiCli { .. } | Self::AntigravityCli { .. } => {
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
                Self::Hermes { .. } => {
                    cmd.arg("--resume").arg(session_id);
                }
                Self::OpenaiCompat { .. } => {} // HTTP-only
                Self::ClaudeCode { .. } => {}
            }
        }

        // Prompt (position matters per tool)
        match prompt_transport {
            PromptTransport::Argv => match self {
                Self::GeminiCli { .. } | Self::ClaudeCode { .. } | Self::AntigravityCli { .. } => {
                    cmd.arg("-p").arg(prompt);
                }
                Self::Opencode { .. } | Self::Codex { .. } => {
                    cmd.arg(prompt);
                }
                Self::Hermes { .. } => {
                    cmd.arg(prompt);
                }
                Self::OpenaiCompat { .. } => {} // HTTP-only
            },
            PromptTransport::Stdin => {
                match self {
                    Self::GeminiCli { .. }
                    | Self::ClaudeCode { .. }
                    | Self::AntigravityCli { .. } => {
                        cmd.arg("-p");
                    }
                    Self::Codex { .. } if codex_resume => {
                        cmd.arg("-");
                    }
                    Self::Opencode { .. }
                    | Self::Codex { .. }
                    | Self::Hermes { .. }
                    | Self::OpenaiCompat { .. } => {
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
                    tracing::debug!(
                        "Ignoring thinking budget for {}: no flag support",
                        self.tool_name()
                    );
                }
            }
            Self::AntigravityCli {
                thinking_budget, ..
            } => {
                // `agy` does NOT accept `-m`; the active model is read from
                // `~/.gemini/antigravity-cli/settings.json` instead. The model
                // override is applied to that file by
                // `AntigravitySettingsGuard::apply_model` in the transport
                // layer just before spawning `agy`, and restored on drop.
                // Because settings.json is a process-wide singleton this also
                // implies an effective `max_concurrent = 1` for antigravity-cli
                // sessions (see #1620).
                if thinking_budget.is_some() {
                    tracing::debug!(
                        "Ignoring thinking budget for {}: no flag support",
                        self.tool_name()
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
                runtime_metadata,
            } => {
                if let Some(model) = model_override {
                    cmd.arg("--model").arg(model);
                }
                if let Some(budget) = thinking_budget {
                    cmd.arg("-c")
                        .arg(format!("model_reasoning_effort={}", budget.codex_effort()));
                }
                if runtime_metadata.fast_mode_enabled() {
                    cmd.arg("--enable").arg("fast_mode");
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
                // claude-code 2.x: `--effort <level>`, not `--thinking-budget`
                // (removed, #1124). DefaultBudget omits the flag entirely.
                if let Some(budget) = thinking_budget
                    && let Some(level) = budget.claude_effort()
                {
                    cmd.arg("--effort").arg(level);
                }
            }
            Self::Hermes {
                provider_override,
                model_override,
                thinking_budget,
            } => {
                let (provider, model) = hermes_dispatch_identity(
                    provider_override.as_deref(),
                    model_override.as_deref(),
                );
                if let Some(provider) = provider {
                    cmd.arg("--provider").arg(provider);
                }
                if let Some(model) = model {
                    cmd.arg("--model").arg(model);
                }
                if let Some(budget) = thinking_budget {
                    cmd.arg("--thinking").arg(budget.token_count().to_string());
                }
            }
            Self::OpenaiCompat { .. } => {} // HTTP-only: model/thinking via API body
        }
    }

    fn append_yolo_args(&self, cmd: &mut Command) {
        for arg in self.yolo_args() {
            cmd.arg(arg);
        }
    }
}

#[cfg(test)]
mod hermes_identity_tests {
    use super::{Command, Executor, hermes_dispatch_identity};

    #[test]
    fn provider_qualified_model_overrides_separate_provider() {
        assert_eq!(
            hermes_dispatch_identity(Some("openai"), Some("anthropic/claude")),
            (Some("anthropic"), Some("claude"))
        );
        let executor = Executor::Hermes {
            provider_override: Some("openai".to_string()),
            model_override: Some("anthropic/claude".to_string()),
            thinking_budget: None,
        };
        let mut command = Command::new("hermes");
        executor.append_model_args(&mut command);
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["--provider", "anthropic", "--model", "claude"]);
    }

    #[test]
    fn bare_model_preserves_separate_provider() {
        assert_eq!(
            hermes_dispatch_identity(Some("openai"), Some("gpt")),
            (Some("openai"), Some("gpt"))
        );
    }
}
