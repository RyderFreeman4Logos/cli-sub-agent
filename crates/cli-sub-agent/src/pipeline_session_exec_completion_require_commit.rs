use csa_core::types::OutputFormat;

use super::session_exec_audit;

pub(super) fn compute_changed_paths_from_snapshots(
    pre_run_workspace: Option<&crate::run_cmd::GitWorkspaceSnapshot>,
    post_run_workspace: Option<&crate::run_cmd::GitWorkspaceSnapshot>,
) -> Vec<String> {
    let pre_fingerprints = pre_run_workspace.map(session_exec_audit::snapshot_to_fingerprints);
    let post_fingerprints = post_run_workspace.map(session_exec_audit::snapshot_to_fingerprints);
    crate::pipeline::changed_paths::compute_changed_paths(
        pre_run_workspace.map(|s| s.status.as_str()),
        post_run_workspace.map(|s| s.status.as_str()),
        pre_fingerprints.as_ref(),
        post_fingerprints.as_ref(),
    )
}

pub(super) fn should_attempt_require_commit_rescue(
    require_commit_on_mutation: bool,
    commit_guard: Option<&crate::run_cmd::PostRunCommitGuard>,
) -> bool {
    require_commit_on_mutation
        && commit_guard.is_some_and(|guard| guard.workspace_mutated && !guard.head_changed)
}

pub(super) fn record_require_commit_rescue(
    output_format: &OutputFormat,
    result: &mut csa_process::ExecutionResult,
    tool_name: &str,
    new_head: &str,
) {
    let message = format!(
        "CSA require-commit rescue: created commit {new_head} for {tool_name} writer using \
         CSA-owned rescue path (`git add -A && git commit --no-verify`)."
    );
    append_result_stderr_block(result, &message);
    if matches!(output_format, OutputFormat::Text) {
        eprintln!("{message}");
    }
}

fn append_result_stderr_block(result: &mut csa_process::ExecutionResult, block: &str) {
    if block.trim().is_empty() {
        return;
    }
    if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
        result.stderr_output.push('\n');
    }
    result.stderr_output.push_str(block);
    if !result.stderr_output.ends_with('\n') {
        result.stderr_output.push('\n');
    }
}
