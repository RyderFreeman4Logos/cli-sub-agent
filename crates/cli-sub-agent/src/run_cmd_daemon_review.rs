use std::path::Path;

use super::{DaemonSpawnOptions, PromptFileForwardArg, RunStdinPrompt};

impl DaemonSpawnOptions {
    pub(crate) fn for_review_fix_finding(prompt: Option<&str>, prompt_file: Option<&Path>) -> Self {
        let prompt_file_is_stdin =
            prompt_file.is_some_and(crate::run_helpers::is_prompt_file_stdin_sentinel);
        let run_stdin_prompt = if prompt_file_is_stdin {
            RunStdinPrompt::PromptFileSentinel
        } else if prompt.is_some() || prompt_file.is_some() {
            RunStdinPrompt::None
        } else {
            RunStdinPrompt::Omitted
        };

        Self {
            run_stdin_prompt,
            prompt_file_forward_arg: PromptFileForwardArg::PromptFile,
            ..Default::default()
        }
    }
}
