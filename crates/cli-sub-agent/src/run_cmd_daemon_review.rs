use std::path::Path;

use super::{DaemonSpawnOptions, PromptFileForwardArg, RunStdinPrompt};

impl DaemonSpawnOptions {
    pub(crate) fn for_review_fix_finding(prompt: Option<&str>, prompt_file: Option<&Path>) -> Self {
        let prompt_file_is_stdin =
            prompt_file.is_some_and(crate::run_helpers::is_prompt_file_stdin_sentinel);
        // When the caller omits --prompt/--prompt-file, the child may derive an
        // unambiguous prompt from the source review's findings.toml (#2654).
        // Do not force a parent-side empty-stdin failure in that case.
        // Explicit `--prompt-file -` still requests stdin via PromptFileSentinel.
        let run_stdin_prompt = if prompt_file_is_stdin {
            RunStdinPrompt::PromptFileSentinel
        } else {
            RunStdinPrompt::None
        };
        let _ = (prompt, prompt_file);

        Self {
            run_stdin_prompt,
            prompt_file_forward_arg: PromptFileForwardArg::PromptFile,
            ..Default::default()
        }
    }
}
