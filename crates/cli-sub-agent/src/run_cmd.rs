//! `csa run` command handler.
//!
//! Extracted from main.rs to keep file sizes manageable.

#[path = "run_cmd_attempt.rs"]
mod attempt;
#[path = "run_cmd_attempt_exec.rs"]
mod attempt_exec;
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
    capture_git_workspace_snapshot, evaluate_post_run_commit_guard, is_git_worktree,
};
pub(crate) use policy::{
    apply_no_verify_commit_policy, apply_post_run_commit_policy, apply_unverifiable_commit_policy,
    execute_tool_calls_observed, extract_executed_shell_commands,
};

#[cfg(test)]
pub(crate) use git::{
    GitWorkspaceSnapshot, PostRunCommitGuard, changed_paths_from_status, tracked_paths_from_status,
};
#[cfg(test)]
pub(crate) use policy::{
    events_contain_execute_tool_calls, extract_executed_shell_commands_from_events,
};
#[cfg(test)]
pub(crate) use policy::{format_post_run_commit_guard_message, is_post_run_commit_policy_block};
#[cfg(test)]
pub(crate) use resume::{
    build_resume_hint_command, extract_meta_session_id_from_error, resolve_run_timeout_seconds,
    session_matches_interrupted_skill, signal_interruption_exit_code, skill_session_description,
    wall_timeout_seconds_from_error,
};

#[cfg(test)]
#[path = "run_cmd_tests.rs"]
mod tests;
