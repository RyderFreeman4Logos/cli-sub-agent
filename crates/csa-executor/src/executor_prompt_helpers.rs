use csa_core::types::{PromptTransport, ToolName, prompt_transport_capabilities};
use csa_session::state::MetaSessionState;
use tokio::process::Command;

use super::{Executor, MAX_ARGV_PROMPT_LEN};

impl Executor {
    /// Generate a CSA sub-agent identity preamble for tools that load project rules
    /// (e.g., claude-code loading AGENTS.md). Without this, the tool may refuse CSA-
    /// formatted prompts by misapplying delegation rules against itself.
    pub(super) fn csa_sub_agent_identity_preamble(session: &MetaSessionState) -> String {
        let session_id = &session.meta_session_id;
        let child_depth = session.genealogy.depth + 1;
        format!(
            "<csa-sub-agent-context>\n\
             You are running INSIDE a CSA (cli-sub-agent) session as the delegated executor.\n\
             CSA_SESSION_ID={session_id}, CSA_DEPTH={child_depth}.\n\
             You ARE the sub-agent — execute the task directly.\n\
             Do NOT re-delegate to `csa run` or suggest CSA dispatch.\n\
             AGENTS.md rule 049 does not apply: you are already the CSA-dispatched agent.\n\
             </csa-sub-agent-context>\n\n"
        )
    }

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
