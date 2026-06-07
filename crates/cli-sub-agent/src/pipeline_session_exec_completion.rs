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
    enforce_result_toml_path_contract(
        input.prompt,
        &input.effective_prompt,
        input.session_dir,
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
    let post_run_workspace = session_exec_audit::capture_git_workspace_snapshot_if_needed(
        is_git,
        input.project_root,
        require_commit_on_mutation,
    );
    let pre_fingerprints = pre_run_workspace
        .as_ref()
        .map(session_exec_audit::snapshot_to_fingerprints);
    let post_fingerprints = post_run_workspace
        .as_ref()
        .map(session_exec_audit::snapshot_to_fingerprints);
    let changed_paths = crate::pipeline::changed_paths::compute_changed_paths(
        pre_run_workspace.as_ref().map(|s| s.status.as_str()),
        post_run_workspace.as_ref().map(|s| s.status.as_str()),
        pre_fingerprints.as_ref(),
        post_fingerprints.as_ref(),
    );
    let snapshots_available = pre_run_workspace.is_some() && post_run_workspace.is_some();
    if commit_guard_enabled {
        let commit_guard = crate::run_cmd::evaluate_post_run_commit_guard(
            pre_run_workspace.as_ref(),
            post_run_workspace.as_ref(),
        );
        let policy_evaluation_failed = require_commit_on_mutation
            && (!inside_git_worktree
                || pre_run_workspace.is_none()
                || post_run_workspace.is_none());
        crate::run_cmd::apply_post_session_commit_policies(
            &mut result,
            crate::run_cmd::PostSessionCommitPolicyArgs {
                output_format: input.output_format,
                prompt: input.prompt,
                require_commit_on_mutation,
                commit_guard: commit_guard.as_ref(),
                policy_evaluation_failed,
                hook_bypass_scan_enabled,
                executed_shell_commands: &executed_shell_commands,
                merged_env_ref,
                execute_events_observed,
            },
        );
    }
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
        has_tool_calls,
        turn_count,
        output_tokens,
        sa_mode,
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
    })
}
