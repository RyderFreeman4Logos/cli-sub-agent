use std::path::Path;

use anyhow::{Context, Result};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_executor::{Executor, TransportResult};
use csa_session::MetaSessionState;

use super::super::result_contract::enforce_result_toml_path_contract;
use super::session_exec_audit;
use super::session_exec_runtime::SessionCompletionPlan;
use super::session_exec_write_guard::apply_write_restriction_violations;
use crate::pipeline::SessionExecutionResult;

#[path = "pipeline_session_exec_completion_require_commit.rs"]
mod require_commit;

const REVIEW_FIX_FINDING_TASK_TYPE: &str = "review_fix_finding";

pub(super) struct CompletionInput<'a> {
    pub(super) executor: &'a Executor,
    pub(super) tool: &'a ToolName,
    pub(super) prompt: &'a str,
    pub(super) output_format: &'a OutputFormat,
    pub(super) task_type: Option<&'a str>,
    pub(super) readonly_project_root: bool,
    pub(super) project_root: &'a Path,
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) global_config: Option<&'a GlobalConfig>,
    pub(super) session_dir: &'a Path,
    pub(super) memory_project_key: Option<String>,
    pub(super) effective_prompt: String,
    pub(super) plan: SessionCompletionPlan,
    pub(super) transport_result: TransportResult,
}

pub(super) async fn complete_session_execution(
    input: CompletionInput<'_>,
    session: &mut MetaSessionState,
) -> Result<SessionExecutionResult> {
    let SessionCompletionPlan {
        merged_env,
        hooks_config,
        sessions_root,
        edit_guard,
        new_file_guard,
        result_file_cleared,
        execution_start_time,
        commit_guard_enabled,
        require_commit_on_mutation,
        hook_bypass_scan_enabled,
        is_git,
        inside_git_worktree,
        pre_run_workspace,
        pre_exec_snapshot,
        timeout_diagnostics,
        sa_mode,
    } = input.plan;
    let merged_env_ref = (!merged_env.is_empty()).then_some(&merged_env);
    let transport_result = input.transport_result;
    let provider_session_id =
        csa_executor::extract_session_id_from_transport(input.tool, &transport_result);
    let events_count = transport_result
        .metadata
        .total_events_count
        .max(transport_result.events.len()) as u64;
    let execute_events_observed = crate::run_cmd::execute_tool_calls_observed(
        &transport_result.metadata,
        &transport_result.events,
    );
    let mut executed_shell_commands = crate::run_cmd::extract_executed_shell_commands(
        &transport_result.metadata,
        &transport_result.events,
    );
    if transport_result.metadata.has_no_verify_commit
        && crate::run_cmd::detect_no_verify_commit_commands(&executed_shell_commands).is_empty()
    {
        executed_shell_commands.push("git commit --no-verify".to_string());
    }
    let transcript_artifacts = crate::pipeline_transcript::persist_if_enabled(
        input.config,
        input.session_dir,
        &transport_result,
    );
    let has_tool_calls = transport_result.metadata.has_tool_calls
        || transport_result.metadata.has_execute_tool_calls;
    let turn_count = transport_result.metadata.turn_count;
    let output_tokens = transport_result.metadata.output_tokens;
    let mut result = transport_result.execution;
    crate::pipeline_sandbox::check_sandbox_permission_errors(
        &result.stderr_output,
        session.sandbox_info.as_ref(),
    );
    // Capture the raw exit code BEFORE result-contract enforcement. Contract
    // validation (below) may call `mark_gate_failure`, coercing `exit_code` to
    // `1`; the dirty-SA exit-preservation path needs the ORIGINAL nonzero code
    // (e.g. timeout 124 / signal 137), not the coerced 1 (#2806 R10-F3).
    let original_exit_code = result.exit_code;
    enforce_result_toml_path_contract(
        input.prompt,
        &input.effective_prompt,
        input.session_dir,
        session.turn_count,
        result_file_cleared,
        &mut result,
    );
    apply_write_restriction_violations(edit_guard, new_file_guard, input.executor, &mut result)?;
    if result.exit_code != 0 {
        crate::error_hints::append_sandbox_fs_denial_hint(
            &mut result.stderr_output,
            &result.output,
            crate::pipeline_sandbox::filesystem_sandbox_active(session.sandbox_info.as_ref()),
            &session.meta_session_id,
        );
    }
    let mut post_run_workspace = session_exec_audit::capture_git_workspace_snapshot_if_needed(
        is_git,
        input.project_root,
        require_commit_on_mutation,
    );
    let mut rescued_changed_paths = None;
    let mut commit_created = None;
    if commit_guard_enabled {
        let is_user_interrupted = result.exit_signal == Some(libc::SIGINT);
        let is_timed_out = result.terminal_reason.as_deref() == Some("timeout");
        let effective_require_commit_on_mutation = require_commit_on_mutation
            && !is_fix_finding_session(session)
            && !is_user_interrupted
            && !is_timed_out;
        commit_created = pre_run_workspace
            .as_ref()
            .zip(post_run_workspace.as_ref())
            .map(|(before, after)| before.head != after.head);
        let mut commit_guard = crate::run_cmd::evaluate_post_run_commit_guard(
            pre_run_workspace.as_ref(),
            post_run_workspace.as_ref(),
        );
        let mut policy_evaluation_failed = effective_require_commit_on_mutation
            && (!inside_git_worktree
                || pre_run_workspace.is_none()
                || post_run_workspace.is_none());
        let git_commit_attempted =
            !crate::run_cmd::detect_git_commit_commands(&executed_shell_commands).is_empty();
        let sandbox_hook_probe = effective_require_commit_on_mutation.then(|| {
            crate::run_cmd::sandbox_commit_failure_matches(
                input.project_root,
                &session.meta_session_id,
            )
        });
        let sandbox_hook_blocked = matches!(sandbox_hook_probe, Some(Ok(true)));
        let sandbox_hook_probe_uncertain = matches!(sandbox_hook_probe, Some(Err(_)))
            || (git_commit_attempted && matches!(sandbox_hook_probe, Some(Ok(false))));
        policy_evaluation_failed |= sandbox_hook_probe_uncertain;
        if !sandbox_hook_blocked
            && !sandbox_hook_probe_uncertain
            && require_commit::should_attempt_require_commit_rescue(
                effective_require_commit_on_mutation,
                commit_guard.as_ref(),
            )
            && let Some(new_head) = crate::run_cmd::attempt_rescue_commit(
                input.project_root,
                input.executor.tool_name(),
            )
        {
            commit_created = Some(true);
            rescued_changed_paths = Some(require_commit::compute_changed_paths_from_snapshots(
                pre_run_workspace.as_ref(),
                post_run_workspace.as_ref(),
            ));
            require_commit::record_require_commit_rescue(
                input.output_format,
                &mut result,
                input.executor.tool_name(),
                &new_head,
            );
            post_run_workspace = session_exec_audit::capture_git_workspace_snapshot_if_needed(
                is_git,
                input.project_root,
                require_commit_on_mutation,
            );
            commit_guard = crate::run_cmd::evaluate_post_run_commit_guard(
                pre_run_workspace.as_ref(),
                post_run_workspace.as_ref(),
            );
            policy_evaluation_failed = effective_require_commit_on_mutation
                && (!inside_git_worktree
                    || pre_run_workspace.is_none()
                    || post_run_workspace.is_none());
        }
        let commit_reflog_race = if git_commit_attempted && commit_created == Some(true) {
            let current_head = post_run_workspace
                .as_ref()
                .and_then(|snap| snap.head.as_deref());
            crate::run_cmd::detect_external_checkout_after_commit(
                input.project_root,
                current_head,
                execution_start_time,
            )
        } else {
            None
        };
        crate::run_cmd::apply_post_session_commit_policies(
            &mut result,
            crate::run_cmd::PostSessionCommitPolicyArgs {
                output_format: input.output_format,
                prompt: input.prompt,
                tool_name: input.executor.tool_name(),
                require_commit_on_mutation: effective_require_commit_on_mutation,
                commit_guard: commit_guard.as_ref(),
                policy_evaluation_failed,
                hook_bypass_scan_enabled,
                executed_shell_commands: &executed_shell_commands,
                commit_reflog_race: commit_reflog_race.as_ref(),
                merged_env_ref,
                execute_events_observed,
            },
        );
        apply_fix_finding_terminal_guard(
            session,
            commit_created,
            commit_guard.as_ref(),
            &mut result,
        );
        apply_fix_finding_terminal_guard_summary(input.project_root, session, &mut result);
    }
    let mut changed_paths = require_commit::compute_changed_paths_from_snapshots(
        pre_run_workspace.as_ref(),
        post_run_workspace.as_ref(),
    );
    if changed_paths.is_empty()
        && let Some(paths) = rescued_changed_paths
    {
        changed_paths = paths;
    }
    let snapshots_available = pre_run_workspace.is_some() && post_run_workspace.is_some();
    let post_ctx = crate::pipeline_post_exec::PostExecContext {
        executor: input.executor,
        prompt: input.prompt,
        effective_prompt: &input.effective_prompt,
        task_type: input.task_type,
        readonly_project_root: input.readonly_project_root,
        project_root: input.project_root,
        config: input.config,
        global_config: input.global_config,
        session_dir: input.session_dir.to_path_buf(),
        sessions_root,
        execution_start_time,
        hooks_config: &hooks_config,
        memory_project_key: input.memory_project_key,
        provider_session_id: provider_session_id.clone(),
        events_count,
        transcript_artifacts,
        changed_paths: changed_paths.clone(),
        pre_exec_snapshot,
        timeout_diagnostics,
        has_tool_calls,
        turn_count,
        output_tokens,
        sa_mode,
        original_exit_code: Some(original_exit_code),
    };
    if let Err(err) =
        crate::pipeline_post_exec::process_execution_result(post_ctx, session, &mut result).await
    {
        crate::pipeline_post_exec::ensure_terminal_result_on_post_exec_error(
            input.project_root,
            session,
            input.executor.tool_name(),
            execution_start_time,
            &err,
        );
        return Err(err).with_context(|| format!("meta_session_id={}", session.meta_session_id));
    }
    Ok(SessionExecutionResult {
        execution: result,
        meta_session_id: session.meta_session_id.clone(),
        provider_session_id,
        changed_paths: snapshots_available.then_some(changed_paths),
        commit_created,
    })
}

fn apply_fix_finding_terminal_guard_summary(
    project_root: &Path,
    session: &MetaSessionState,
    result: &mut csa_process::ExecutionResult,
) {
    if !is_fix_finding_session(session) || !is_fix_finding_terminal_guard_failure(result) {
        return;
    }

    let reason = result
        .csa_gate_failure
        .as_deref()
        .unwrap_or("fix-finding-terminal-guard");
    let side_effects =
        crate::session_fix_finding_recovery::side_effect_diagnostic(project_root, session);
    let failure_detail = fix_finding_terminal_failure_detail(reason);
    let summary = format!(
        "`csa review --fix-finding` session failed closed: {failure_detail} \
         (reason={reason}). {side_effects}. Recovery: inspect `git status --short`, `git diff`, and \
         `git diff --staged`; preserve/finish or discard dirty side effects; create a \
         hook-enabled amend/commit if appropriate; then run a fresh exact-head \
         `csa review` before push/PR."
    );
    result.summary = summary.clone();
    if !result.stderr_output.contains(&summary) {
        if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        result.stderr_output.push_str(&summary);
        result.stderr_output.push('\n');
    }
}

fn is_fix_finding_terminal_guard_failure(result: &csa_process::ExecutionResult) -> bool {
    result.csa_gate_failure.as_deref().is_some_and(|reason| {
        reason == "fix-finding-no-change"
            || crate::run_cmd::is_post_run_commit_policy_gate_failure(result)
    })
}

fn fix_finding_terminal_failure_detail(reason: &str) -> &'static str {
    match reason {
        "fix-finding-no-change" => "child reported success but no repository changes were detected",
        "commit-policy-ref-update" => "git commit was attempted but HEAD did not advance cleanly",
        "commit-policy-no-verify" => "forbidden git commit --no-verify was detected",
        "commit-policy-lefthook-bypass" => "forbidden LEFTHOOK bypass was detected during the fix",
        "commit-policy-unverifiable" => "CSA could not verify the repository mutation state",
        "commit-policy-uncommitted" => {
            "a strict commit policy required committed changes but dirty work remained"
        }
        _ => "a terminal fix-finding policy failed",
    }
}

fn apply_fix_finding_terminal_guard(
    session: &MetaSessionState,
    commit_created: Option<bool>,
    commit_guard: Option<&crate::run_cmd::PostRunCommitGuard>,
    result: &mut csa_process::ExecutionResult,
) {
    if !is_fix_finding_session(session)
        || result.exit_code != 0
        || crate::run_cmd::is_post_run_commit_policy_gate_failure(result)
    {
        return;
    }

    if commit_guard.is_some_and(|guard| guard.workspace_mutated) {
        return;
    }

    if commit_created == Some(false) {
        result.note_gate_failure("fix-finding-no-change");
    }
}

fn is_fix_finding_session(session: &MetaSessionState) -> bool {
    session.task_context.task_type.as_deref() == Some(REVIEW_FIX_FINDING_TASK_TYPE)
}

#[cfg(test)]
#[path = "pipeline_session_exec_completion_fix_finding_tests.rs"]
mod fix_finding_tests;

#[cfg(test)]
#[path = "pipeline_session_exec_completion_require_commit_tests.rs"]
mod require_commit_tests;

#[cfg(test)]
#[path = "pipeline_session_exec_completion_autofix_hook_tests.rs"]
mod autofix_hook_tests;

#[cfg(test)]
#[path = "pipeline_session_exec_completion_tests.rs"]
mod tests;
