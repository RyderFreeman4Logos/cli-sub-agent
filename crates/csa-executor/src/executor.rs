//! Executor enum for 4 AI tools.

use anyhow::{bail, Result};
use csa_core::types::ToolName;
use csa_process::ExecutionResult;
use csa_session::state::{MetaSessionState, ToolState};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Command;

use crate::model_spec::{ModelSpec, ThinkingBudget};

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

    /// Get the executable name for the tool.
    pub fn executable_name(&self) -> &'static str {
        match self {
            Self::GeminiCli { .. } => "gemini",
            Self::Opencode { .. } => "opencode",
            Self::Codex { .. } => "codex",
            Self::ClaudeCode { .. } => "claude",
        }
    }

    /// Get installation instructions for the tool.
    pub fn install_hint(&self) -> &'static str {
        match self {
            Self::GeminiCli { .. } => "Install: npm install -g @anthropic-ai/gemini-cli",
            Self::Opencode { .. } => "Install: go install github.com/anthropics/opencode@latest",
            Self::Codex { .. } => "Install: npm install -g @openai/codex",
            Self::ClaudeCode { .. } => "Install: npm install -g @anthropic-ai/claude-code",
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

    /// Construct executor from ToolName enum.
    pub fn from_tool_name(tool: &ToolName, model: Option<String>) -> Self {
        match tool {
            ToolName::GeminiCli => Self::GeminiCli {
                model_override: model,
                thinking_budget: None,
            },
            ToolName::Opencode => Self::Opencode {
                model_override: model,
                agent: None,
                thinking_budget: None,
            },
            ToolName::Codex => Self::Codex {
                model_override: model,
                thinking_budget: None,
            },
            ToolName::ClaudeCode => Self::ClaudeCode {
                model_override: model,
                thinking_budget: None,
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

    /// Build a configured Command ready for execution.
    ///
    /// Returns the Command object without executing it, allowing caller to:
    /// - Spawn the process and get its PID
    /// - Start resource monitoring
    /// - Wait for completion separately
    ///
    /// This is useful when you need to monitor the child process (e.g., memory usage).
    pub fn build_command(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
    ) -> Command {
        let mut cmd = self.build_base_command(session);
        self.append_tool_args(&mut cmd, prompt, tool_state);
        cmd
    }

    /// Execute a task with full session context.
    pub async fn execute(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
    ) -> Result<ExecutionResult> {
        let cmd = self.build_command(prompt, tool_state, session);
        csa_process::run_and_capture(cmd).await
    }

    /// Execute in a specific directory (for ephemeral sessions).
    pub async fn execute_in(&self, prompt: &str, work_dir: &Path) -> Result<ExecutionResult> {
        let mut cmd = Command::new(self.executable_name());
        cmd.current_dir(work_dir);
        self.append_yolo_args(&mut cmd);
        self.append_prompt_args(&mut cmd, prompt);
        csa_process::run_and_capture(cmd).await
    }

    /// Build base command with session environment variables.
    fn build_base_command(&self, session: &MetaSessionState) -> Command {
        let mut cmd = Command::new(self.executable_name());
        cmd.current_dir(&session.project_path);

        // Set CSA environment variables
        cmd.env("CSA_SESSION_ID", &session.meta_session_id);
        cmd.env("CSA_DEPTH", (session.genealogy.depth + 1).to_string());
        cmd.env("CSA_PROJECT_ROOT", &session.project_path);

        if let Some(ref parent) = session.genealogy.parent_session_id {
            cmd.env("CSA_PARENT_SESSION", parent);
        }

        cmd
    }

    /// Append tool-specific arguments for full execution.
    fn append_tool_args(&self, cmd: &mut Command, prompt: &str, tool_state: Option<&ToolState>) {
        match self {
            Self::GeminiCli {
                model_override,
                thinking_budget,
            } => {
                cmd.arg("-p").arg(prompt);
                if let Some(model) = model_override {
                    cmd.arg("-m").arg(model);
                }
                if let Some(budget) = thinking_budget {
                    cmd.arg("--thinking_budget")
                        .arg(budget.token_count().to_string());
                }
                cmd.arg("-y");
                if let Some(state) = tool_state {
                    if let Some(ref session_id) = state.provider_session_id {
                        cmd.arg("-r").arg(session_id);
                    }
                }
            }
            Self::Opencode {
                model_override,
                agent,
                thinking_budget,
            } => {
                cmd.arg("run");
                cmd.arg("--format").arg("json");
                if let Some(model) = model_override {
                    cmd.arg("-m").arg(model);
                }
                if let Some(agent_name) = agent {
                    cmd.arg("--agent").arg(agent_name);
                }
                // Map thinking budget to --variant (opencode's reasoning effort parameter)
                if let Some(budget) = thinking_budget {
                    let variant = match budget {
                        ThinkingBudget::Low => "minimal",
                        ThinkingBudget::Medium => "medium",
                        ThinkingBudget::High => "high",
                        ThinkingBudget::Xhigh => "max",
                        ThinkingBudget::Custom(_) => "max",
                    };
                    cmd.arg("--variant").arg(variant);
                }
                if let Some(state) = tool_state {
                    if let Some(ref session_id) = state.provider_session_id {
                        cmd.arg("-s").arg(session_id);
                    }
                }
                cmd.arg(prompt);
            }
            Self::Codex {
                model_override,
                thinking_budget,
            } => {
                cmd.arg("exec");
                cmd.arg("--dangerously-bypass-approvals-and-sandbox");
                if let Some(model) = model_override {
                    cmd.arg("--model").arg(model);
                }
                if let Some(budget) = thinking_budget {
                    cmd.arg("--reasoning-effort").arg(budget.codex_effort());
                }
                if let Some(state) = tool_state {
                    if let Some(ref session_id) = state.provider_session_id {
                        cmd.arg("--session-id").arg(session_id);
                    }
                }
                cmd.arg(prompt);
            }
            Self::ClaudeCode {
                model_override,
                thinking_budget,
            } => {
                cmd.arg("--dangerously-skip-permissions");
                cmd.arg("--output-format").arg("json");
                if let Some(model) = model_override {
                    cmd.arg("--model").arg(model);
                }
                if let Some(budget) = thinking_budget {
                    cmd.arg("--thinking-budget")
                        .arg(budget.token_count().to_string());
                }
                if let Some(state) = tool_state {
                    if let Some(ref session_id) = state.provider_session_id {
                        cmd.arg("--resume").arg(session_id);
                    }
                }
                cmd.arg("-p").arg(prompt);
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
        match self {
            Self::GeminiCli { .. } => {
                cmd.arg("-p").arg(prompt);
            }
            Self::Opencode { .. } => {
                cmd.arg("run").arg(prompt);
            }
            Self::Codex { .. } => {
                cmd.arg("exec").arg(prompt);
            }
            Self::ClaudeCode { .. } => {
                cmd.arg("-p").arg(prompt);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_name() {
        assert_eq!(
            Executor::GeminiCli {
                model_override: None,
                thinking_budget: None,
            }
            .tool_name(),
            "gemini-cli"
        );
        assert_eq!(
            Executor::Opencode {
                model_override: None,
                agent: None,
                thinking_budget: None,
            }
            .tool_name(),
            "opencode"
        );
        assert_eq!(
            Executor::Codex {
                model_override: None,
                thinking_budget: None,
            }
            .tool_name(),
            "codex"
        );
        assert_eq!(
            Executor::ClaudeCode {
                model_override: None,
                thinking_budget: None,
            }
            .tool_name(),
            "claude-code"
        );
    }

    #[test]
    fn test_executable_name() {
        assert_eq!(
            Executor::GeminiCli {
                model_override: None,
                thinking_budget: None,
            }
            .executable_name(),
            "gemini"
        );
        assert_eq!(
            Executor::Opencode {
                model_override: None,
                agent: None,
                thinking_budget: None,
            }
            .executable_name(),
            "opencode"
        );
        assert_eq!(
            Executor::Codex {
                model_override: None,
                thinking_budget: None,
            }
            .executable_name(),
            "codex"
        );
        assert_eq!(
            Executor::ClaudeCode {
                model_override: None,
                thinking_budget: None,
            }
            .executable_name(),
            "claude"
        );
    }

    #[test]
    fn test_install_hint() {
        assert_eq!(
            Executor::GeminiCli {
                model_override: None,
                thinking_budget: None,
            }
            .install_hint(),
            "Install: npm install -g @anthropic-ai/gemini-cli"
        );
        assert_eq!(
            Executor::Opencode {
                model_override: None,
                agent: None,
                thinking_budget: None,
            }
            .install_hint(),
            "Install: go install github.com/anthropics/opencode@latest"
        );
        assert_eq!(
            Executor::Codex {
                model_override: None,
                thinking_budget: None,
            }
            .install_hint(),
            "Install: npm install -g @openai/codex"
        );
        assert_eq!(
            Executor::ClaudeCode {
                model_override: None,
                thinking_budget: None,
            }
            .install_hint(),
            "Install: npm install -g @anthropic-ai/claude-code"
        );
    }

    #[test]
    fn test_yolo_args() {
        assert_eq!(
            Executor::GeminiCli {
                model_override: None,
                thinking_budget: None,
            }
            .yolo_args(),
            &["-y"]
        );
        assert_eq!(
            Executor::Opencode {
                model_override: None,
                agent: None,
                thinking_budget: None,
            }
            .yolo_args(),
            &[] as &[&str] // opencode does not have a yolo mode
        );
        assert_eq!(
            Executor::Codex {
                model_override: None,
                thinking_budget: None,
            }
            .yolo_args(),
            &["--dangerously-bypass-approvals-and-sandbox"]
        );
        assert_eq!(
            Executor::ClaudeCode {
                model_override: None,
                thinking_budget: None,
            }
            .yolo_args(),
            &["--dangerously-skip-permissions"]
        );
    }

    #[test]
    fn test_from_tool_name() {
        let exec = Executor::from_tool_name(&ToolName::GeminiCli, Some("model-1".to_string()));
        assert_eq!(exec.tool_name(), "gemini-cli");
        assert!(matches!(
            exec,
            Executor::GeminiCli {
                model_override: Some(_),
                thinking_budget: None,
            }
        ));

        let exec = Executor::from_tool_name(&ToolName::Opencode, None);
        assert_eq!(exec.tool_name(), "opencode");
        assert!(matches!(
            exec,
            Executor::Opencode {
                model_override: None,
                agent: None,
                thinking_budget: None,
            }
        ));

        let exec = Executor::from_tool_name(&ToolName::Codex, Some("model-2".to_string()));
        assert_eq!(exec.tool_name(), "codex");
        assert!(matches!(
            exec,
            Executor::Codex {
                model_override: Some(_),
                thinking_budget: None,
            }
        ));

        let exec = Executor::from_tool_name(&ToolName::ClaudeCode, None);
        assert_eq!(exec.tool_name(), "claude-code");
        assert!(matches!(
            exec,
            Executor::ClaudeCode {
                model_override: None,
                thinking_budget: None,
            }
        ));
    }

    #[test]
    fn test_from_spec() {
        let spec = ModelSpec::parse("opencode/google/gemini-2.5-pro/high").unwrap();
        let exec = Executor::from_spec(&spec).unwrap();
        assert_eq!(exec.tool_name(), "opencode");
        assert!(matches!(
            exec,
            Executor::Opencode {
                model_override: Some(_),
                agent: None,
                thinking_budget: Some(_),
            }
        ));

        let spec = ModelSpec::parse("codex/anthropic/claude-opus/medium").unwrap();
        let exec = Executor::from_spec(&spec).unwrap();
        assert_eq!(exec.tool_name(), "codex");
        assert!(matches!(
            exec,
            Executor::Codex {
                model_override: Some(_),
                thinking_budget: Some(_),
            }
        ));
    }

    #[test]
    fn test_from_spec_unknown_tool() {
        let spec = ModelSpec::parse("unknown-tool/provider/model/high").unwrap();
        let result = Executor::from_spec(&spec);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown tool"));
    }

    #[test]
    fn test_thinking_budget_in_gemini_cli_args() {
        use crate::model_spec::ThinkingBudget;
        let exec = Executor::GeminiCli {
            model_override: Some("gemini-3-pro".to_string()),
            thinking_budget: Some(ThinkingBudget::High),
        };

        let mut cmd = Command::new(exec.executable_name());
        exec.append_tool_args(&mut cmd, "test prompt", None);

        // Check that the command contains --thinking_budget 32768
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("--thinking_budget"));
        assert!(debug_str.contains("32768"));
    }

    #[test]
    fn test_thinking_budget_in_codex_args() {
        use crate::model_spec::ThinkingBudget;
        let exec = Executor::Codex {
            model_override: Some("gpt-5".to_string()),
            thinking_budget: Some(ThinkingBudget::Low),
        };

        let mut cmd = Command::new(exec.executable_name());
        exec.append_tool_args(&mut cmd, "test prompt", None);

        // Check that the command contains --reasoning-effort low
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("--reasoning-effort"));
        assert!(debug_str.contains("\"low\""));
    }

    #[test]
    fn test_thinking_budget_from_spec_gemini() {
        let spec = ModelSpec::parse("gemini-cli/google/gemini-3-pro/high").unwrap();
        let exec = Executor::from_spec(&spec).unwrap();

        let mut cmd = Command::new(exec.executable_name());
        exec.append_tool_args(&mut cmd, "test prompt", None);

        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("--thinking_budget"));
        assert!(debug_str.contains("32768"));
    }

    #[test]
    fn test_thinking_budget_from_spec_codex() {
        let spec = ModelSpec::parse("codex/openai/gpt-5/low").unwrap();
        let exec = Executor::from_spec(&spec).unwrap();

        let mut cmd = Command::new(exec.executable_name());
        exec.append_tool_args(&mut cmd, "test prompt", None);

        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("--reasoning-effort"));
        assert!(debug_str.contains("\"low\""));
    }

    #[test]
    fn test_thinking_budget_custom_value() {
        use crate::model_spec::ThinkingBudget;
        let exec = Executor::ClaudeCode {
            model_override: Some("claude-opus".to_string()),
            thinking_budget: Some(ThinkingBudget::Custom(10000)),
        };

        let mut cmd = Command::new(exec.executable_name());
        exec.append_tool_args(&mut cmd, "test prompt", None);

        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("--thinking-budget"));
        assert!(debug_str.contains("10000"));
    }

    #[test]
    fn test_apply_restrictions_allow_edit() {
        let exec = Executor::GeminiCli {
            model_override: None,
            thinking_budget: None,
        };

        let original_prompt = "Refactor the authentication module";
        let result = exec.apply_restrictions(original_prompt, true);

        // When edit is allowed, prompt should be unchanged
        assert_eq!(result, original_prompt);
    }

    #[test]
    fn test_apply_restrictions_deny_edit() {
        let exec = Executor::GeminiCli {
            model_override: None,
            thinking_budget: None,
        };

        let original_prompt = "Analyze the authentication module";
        let result = exec.apply_restrictions(original_prompt, false);

        // When edit is denied, prompt should contain restriction message
        assert!(result.contains("IMPORTANT RESTRICTION"));
        assert!(result.contains("MUST NOT edit or modify any existing files"));
        assert!(result.contains("may only create new files"));
        assert!(result.contains(original_prompt));
    }

    #[test]
    fn test_apply_restrictions_preserves_all_tools() {
        let tools = vec![
            Executor::GeminiCli {
                model_override: None,
                thinking_budget: None,
            },
            Executor::Opencode {
                model_override: None,
                agent: None,
                thinking_budget: None,
            },
            Executor::Codex {
                model_override: None,
                thinking_budget: None,
            },
            Executor::ClaudeCode {
                model_override: None,
                thinking_budget: None,
            },
        ];

        let original_prompt = "Analyze code";
        for tool in tools {
            // Test that restriction works for all tool types
            let restricted = tool.apply_restrictions(original_prompt, false);
            assert!(restricted.contains("IMPORTANT RESTRICTION"));
            assert!(restricted.contains(original_prompt));

            // Test that allowing edit returns original prompt
            let allowed = tool.apply_restrictions(original_prompt, true);
            assert_eq!(allowed, original_prompt);
        }
    }

    #[test]
    fn test_opencode_command_construction() {
        use crate::model_spec::ThinkingBudget;
        let exec = Executor::Opencode {
            model_override: Some("google/gemini-2.5-pro".to_string()),
            agent: Some("test-agent".to_string()),
            thinking_budget: Some(ThinkingBudget::High),
        };

        let mut cmd = Command::new(exec.executable_name());
        exec.append_tool_args(&mut cmd, "test prompt", None);

        // Verify command structure matches opencode run syntax
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("\"run\""));
        assert!(debug_str.contains("\"--format\""));
        assert!(debug_str.contains("\"json\""));
        assert!(debug_str.contains("\"-m\""));
        assert!(debug_str.contains("\"google/gemini-2.5-pro\""));
        assert!(debug_str.contains("\"--agent\""));
        assert!(debug_str.contains("\"test-agent\""));
        assert!(debug_str.contains("\"--variant\""));
        assert!(debug_str.contains("\"high\""));
        assert!(debug_str.contains("\"test prompt\""));
        // Verify --yolo is NOT present
        assert!(!debug_str.contains("--yolo"));
    }

    #[test]
    fn test_opencode_variant_mapping() {
        use crate::model_spec::ThinkingBudget;
        let test_cases = vec![
            (ThinkingBudget::Low, "minimal"),
            (ThinkingBudget::Medium, "medium"),
            (ThinkingBudget::High, "high"),
            (ThinkingBudget::Custom(50000), "max"),
        ];

        for (budget, expected_variant) in test_cases {
            let exec = Executor::Opencode {
                model_override: None,
                agent: None,
                thinking_budget: Some(budget),
            };

            let mut cmd = Command::new(exec.executable_name());
            exec.append_tool_args(&mut cmd, "test", None);

            let debug_str = format!("{:?}", cmd);
            assert!(
                debug_str.contains(expected_variant),
                "Expected variant '{}' not found in command: {}",
                expected_variant,
                debug_str
            );
        }
    }
}
