//! `csa run` command handler.
//!
//! Extracted from main.rs to keep file sizes manageable.

use anyhow::Result;
use csa_core::types::{OutputFormat, ToolArg};

#[path = "run_cmd_attempt.rs"]
mod attempt;
#[path = "run_cmd_attempt_exec.rs"]
mod attempt_exec;
#[path = "run_cmd_attempt_support.rs"]
mod attempt_support;
#[path = "run_cmd_execute.rs"]
mod execute;
#[path = "run_cmd_git.rs"]
mod git;
#[path = "run_cmd_policy.rs"]
mod policy;
#[path = "run_cmd_resume.rs"]
mod resume;
#[path = "run_cmd_shell.rs"]
mod shell;

pub(crate) use execute::handle_run;
pub(crate) use git::{
    GitWorkspaceSnapshot, capture_git_workspace_snapshot, evaluate_post_run_commit_guard,
    is_git_worktree,
};
pub(crate) use policy::{
    apply_lefthook_bypass_policy, apply_no_verify_commit_policy, apply_post_run_commit_policy,
    apply_unverifiable_commit_policy, execute_tool_calls_observed, extract_executed_shell_commands,
};

#[cfg(test)]
pub(crate) use git::{PostRunCommitGuard, changed_paths_from_status, tracked_paths_from_status};
#[cfg(test)]
pub(crate) use policy::{
    events_contain_execute_tool_calls, extract_executed_shell_commands_from_events,
};
#[cfg(test)]
pub(crate) use policy::{format_post_run_commit_guard_message, is_post_run_commit_policy_block};
#[cfg(test)]
pub(crate) use resume::{
    build_resume_hint_command, extract_meta_session_id_from_error,
    interrupted_skill_session_matches_current_vcs, resolve_run_timeout_seconds,
    run_error_timeout_seconds, session_matches_interrupted_skill, signal_interruption_exit_code,
    skill_session_description, wall_timeout_seconds_from_error,
};

#[derive(Debug)]
pub(crate) struct SubagentRunConfig {
    tool: Option<ToolArg>,
    prompt: String,
    timeout: Option<u64>,
    allow_base_branch_working: bool,
    current_depth: u32,
    output_format: OutputFormat,
    stream_mode: csa_process::StreamMode,
}

impl SubagentRunConfig {
    pub(crate) fn new(prompt: String, output_format: OutputFormat) -> Self {
        let stream_mode = match output_format {
            OutputFormat::Text => csa_process::StreamMode::TeeToStderr,
            OutputFormat::Json => csa_process::StreamMode::BufferOnly,
        };

        Self {
            tool: None,
            prompt,
            timeout: None,
            allow_base_branch_working: false,
            current_depth: 0,
            output_format,
            stream_mode,
        }
    }

    pub(crate) fn tool(mut self, tool: Option<ToolArg>) -> Self {
        self.tool = tool;
        self
    }

    pub(crate) fn timeout(mut self, timeout: u64) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub(crate) fn allow_base_branch_working(mut self, allow_base_branch_working: bool) -> Self {
        self.allow_base_branch_working = allow_base_branch_working;
        self
    }

    pub(crate) fn current_depth(mut self, current_depth: u32) -> Self {
        self.current_depth = current_depth;
        self
    }

    pub(crate) async fn run(self) -> Result<i32> {
        handle_run(
            self.tool,
            None,
            None,
            None,
            Some(self.prompt),
            None,
            None,
            None,
            None,
            false,
            None,
            false,
            None,
            false,
            None,
            None,
            false,
            self.allow_base_branch_working,
            None,
            None,
            None,
            None,
            false,
            false,
            false,
            false,
            None,
            None,
            self.timeout,
            false,
            false,
            None,
            self.current_depth,
            self.output_format,
            self.stream_mode,
            None,
            false,
            false,
            Vec::new(),
            Vec::new(),
        )
        .await
    }
}

#[cfg(test)]
#[path = "run_cmd_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "run_cmd_resume_tests.rs"]
mod resume_tests;
