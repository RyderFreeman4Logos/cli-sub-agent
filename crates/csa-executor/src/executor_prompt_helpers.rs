use csa_core::types::{PromptTransport, ToolName, prompt_transport_capabilities};
use tokio::process::Command;

use super::{Executor, MAX_ARGV_PROMPT_LEN};

impl Executor {
    /// Append minimal prompt args for execute_in.
    pub(super) fn append_prompt_args(&self, cmd: &mut Command, prompt: &str) {
        self.append_prompt_args_with_transport(cmd, prompt, PromptTransport::Argv);
    }

    pub(super) fn append_prompt_args_with_transport(
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

    pub(super) fn select_prompt_transport(
        &self,
        prompt: &str,
    ) -> (PromptTransport, Option<Vec<u8>>) {
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
