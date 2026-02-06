//! Executor enum for 4 AI tools.

use anyhow::{bail, Result};
use csa_core::types::ToolName;
use csa_process::ExecutionResult;
use csa_session::state::{MetaSessionState, ToolState};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Command;

use crate::model_spec::ModelSpec;

/// Executor: Closed enum for 4 AI tools.
///
/// Uses data enum pattern (not trait + dynamic dispatch) for a fixed set of tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "kebab-case")]
pub enum Executor {
    GeminiCli {
        model_override: Option<String>,
    },
    Opencode {
        model_override: Option<String>,
        agent: Option<String>,
    },
    Codex {
        model_override: Option<String>,
    },
    ClaudeCode {
        model_override: Option<String>,
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

    /// Get "yolo" args for the tool (bypass approval prompts).
    pub fn yolo_args(&self) -> &[&str] {
        match self {
            Self::GeminiCli { .. } => &["-y"],
            Self::Opencode { .. } => &["--yolo"],
            Self::Codex { .. } => &["--dangerously-bypass-approvals-and-sandbox"],
            Self::ClaudeCode { .. } => &["--dangerously-skip-permissions"],
        }
    }

    /// Construct executor from model spec.
    pub fn from_spec(spec: &ModelSpec) -> Result<Self> {
        let model = Some(spec.model.clone());
        match spec.tool.as_str() {
            "gemini-cli" => Ok(Self::GeminiCli {
                model_override: model,
            }),
            "opencode" => Ok(Self::Opencode {
                model_override: model,
                agent: None,
            }),
            "codex" => Ok(Self::Codex {
                model_override: model,
            }),
            "claude-code" => Ok(Self::ClaudeCode {
                model_override: model,
            }),
            other => bail!("Unknown tool '{}' in model spec", other),
        }
    }

    /// Construct executor from ToolName enum.
    pub fn from_tool_name(tool: &ToolName, model: Option<String>) -> Self {
        match tool {
            ToolName::GeminiCli => Self::GeminiCli {
                model_override: model,
            },
            ToolName::Opencode => Self::Opencode {
                model_override: model,
                agent: None,
            },
            ToolName::Codex => Self::Codex {
                model_override: model,
            },
            ToolName::ClaudeCode => Self::ClaudeCode {
                model_override: model,
            },
        }
    }

    /// Execute a task with full session context.
    pub async fn execute(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
    ) -> Result<ExecutionResult> {
        let mut cmd = self.build_base_command(session);
        self.append_tool_args(&mut cmd, prompt, tool_state);
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
            Self::GeminiCli { model_override } => {
                cmd.arg("-p").arg(prompt);
                if let Some(model) = model_override {
                    cmd.arg("-m").arg(model);
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
            } => {
                cmd.arg("run").arg("--format").arg("json");
                cmd.arg("--yolo");
                if let Some(model) = model_override {
                    cmd.arg("-m").arg(model);
                }
                if let Some(agent_name) = agent {
                    cmd.arg("--agent").arg(agent_name);
                }
                if let Some(state) = tool_state {
                    if let Some(ref session_id) = state.provider_session_id {
                        cmd.arg("-s").arg(session_id);
                    }
                }
                cmd.arg(prompt);
            }
            Self::Codex { model_override } => {
                cmd.arg("exec");
                cmd.arg("--dangerously-bypass-approvals-and-sandbox");
                if let Some(model) = model_override {
                    cmd.arg("--model").arg(model);
                }
                if let Some(state) = tool_state {
                    if let Some(ref session_id) = state.provider_session_id {
                        cmd.arg("--session-id").arg(session_id);
                    }
                }
                cmd.arg(prompt);
            }
            Self::ClaudeCode { model_override } => {
                cmd.arg("--dangerously-skip-permissions");
                cmd.arg("--output-format").arg("json");
                if let Some(model) = model_override {
                    cmd.arg("--model").arg(model);
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
                model_override: None
            }
            .tool_name(),
            "gemini-cli"
        );
        assert_eq!(
            Executor::Opencode {
                model_override: None,
                agent: None
            }
            .tool_name(),
            "opencode"
        );
        assert_eq!(
            Executor::Codex {
                model_override: None
            }
            .tool_name(),
            "codex"
        );
        assert_eq!(
            Executor::ClaudeCode {
                model_override: None
            }
            .tool_name(),
            "claude-code"
        );
    }

    #[test]
    fn test_executable_name() {
        assert_eq!(
            Executor::GeminiCli {
                model_override: None
            }
            .executable_name(),
            "gemini"
        );
        assert_eq!(
            Executor::Opencode {
                model_override: None,
                agent: None
            }
            .executable_name(),
            "opencode"
        );
        assert_eq!(
            Executor::Codex {
                model_override: None
            }
            .executable_name(),
            "codex"
        );
        assert_eq!(
            Executor::ClaudeCode {
                model_override: None
            }
            .executable_name(),
            "claude"
        );
    }

    #[test]
    fn test_yolo_args() {
        assert_eq!(
            Executor::GeminiCli {
                model_override: None
            }
            .yolo_args(),
            &["-y"]
        );
        assert_eq!(
            Executor::Opencode {
                model_override: None,
                agent: None
            }
            .yolo_args(),
            &["--yolo"]
        );
        assert_eq!(
            Executor::Codex {
                model_override: None
            }
            .yolo_args(),
            &["--dangerously-bypass-approvals-and-sandbox"]
        );
        assert_eq!(
            Executor::ClaudeCode {
                model_override: None
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
                model_override: Some(_)
            }
        ));

        let exec = Executor::from_tool_name(&ToolName::Opencode, None);
        assert_eq!(exec.tool_name(), "opencode");
        assert!(matches!(
            exec,
            Executor::Opencode {
                model_override: None,
                agent: None
            }
        ));

        let exec = Executor::from_tool_name(&ToolName::Codex, Some("model-2".to_string()));
        assert_eq!(exec.tool_name(), "codex");
        assert!(matches!(
            exec,
            Executor::Codex {
                model_override: Some(_)
            }
        ));

        let exec = Executor::from_tool_name(&ToolName::ClaudeCode, None);
        assert_eq!(exec.tool_name(), "claude-code");
        assert!(matches!(
            exec,
            Executor::ClaudeCode {
                model_override: None
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
                agent: None
            }
        ));

        let spec = ModelSpec::parse("codex/anthropic/claude-opus/medium").unwrap();
        let exec = Executor::from_spec(&spec).unwrap();
        assert_eq!(exec.tool_name(), "codex");
        assert!(matches!(
            exec,
            Executor::Codex {
                model_override: Some(_)
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
}
