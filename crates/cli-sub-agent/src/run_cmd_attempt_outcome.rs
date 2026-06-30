//! Attempt outcome handling for the `csa run` execution loop.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use tracing::warn;

use super::super::attempt_support::{
    build_failover_context_addendum, merge_retry_changed_paths,
    persist_fork_timeout_result_if_missing,
};
use super::super::policy::is_post_run_commit_policy_gate_failure;
use super::super::resume::{
    build_resume_hint_command, emit_run_timeout, extract_meta_session_id_from_error,
    run_error_timeout_seconds, signal_interruption_exit_code, signal_name_from_exit_code,
};
use crate::run_cmd_fork::{ForkResolution, cleanup_pre_created_fork_session};
use crate::run_cmd_post::{
    ErrorRateLimitFailoverRequest, RateLimitAction, RateLimitFailoverRequest,
    detect_permanent_tool_exhaustion_result, evaluate_error_rate_limit_failover_with_global_config,
    evaluate_rate_limit_failover_with_global_config, is_permanent_tool_exhaustion_error,
};
use crate::run_cmd_tool_selection::take_next_runtime_fallback_tool;

pub(super) enum FailoverContextUpdate {
    Preserve,
    Replace(Option<String>),
}

pub(super) enum AttemptRetryAction {
    Retry {
        new_tool: ToolName,
        new_model_spec: Option<String>,
        failover_context: FailoverContextUpdate,
    },
}

pub(super) enum AttemptErrorAction {
    Exit(i32),
    Retry(AttemptRetryAction),
    Error(anyhow::Error),
}

pub(super) struct AttemptRetryState<'a> {
    pub(super) failover_context: &'a mut Option<String>,
    pub(super) tool: &'a mut ToolName,
    pub(super) model_spec: &'a mut Option<String>,
    pub(super) model: &'a mut Option<String>,
    pub(super) fork_resolution: &'a mut Option<ForkResolution>,
    pub(super) effective_session: &'a mut Option<String>,
    pub(super) is_fork: bool,
}

impl AttemptRetryState<'_> {
    pub(super) fn apply(self, action: AttemptRetryAction) {
        let AttemptRetryAction::Retry {
            new_tool,
            new_model_spec,
            failover_context,
        } = action;

        if let FailoverContextUpdate::Replace(new_context) = failover_context {
            *self.failover_context = new_context;
        }
        *self.tool = new_tool;
        *self.model_spec = new_model_spec;
        *self.model = None;
        *self.fork_resolution = None;
        if self.is_fork {
            *self.effective_session = None;
        }
    }
}

pub(super) struct AttemptErrorRequest<'a> {
    pub(super) run_timeout_seconds: Option<u64>,
    pub(super) project_root: &'a Path,
    pub(super) is_fork: bool,
    pub(super) current_tool: ToolName,
    pub(super) skill: Option<&'a str>,
    pub(super) output_format: OutputFormat,
    pub(super) executed_session_id: Option<&'a str>,
    pub(super) effective_session_arg: Option<&'a str>,
    pub(super) runtime_fallback_enabled: bool,
    pub(super) max_runtime_fallback_attempts: u8,
    pub(super) tool_name: &'a str,
    pub(super) current_model_spec: Option<&'a str>,
    pub(super) attempts: usize,
    pub(super) max_failover_attempts: usize,
    pub(super) tier_auto_select: bool,
    pub(super) failover_on_crash_enabled: bool,
    pub(super) resolved_tier_name: Option<&'a str>,
    pub(super) tier_failover_tool_filter: Option<ToolName>,
    pub(super) ephemeral: bool,
    pub(super) prompt_text: &'a str,
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) global_config: &'a GlobalConfig,
    pub(super) task_needs_edit: Option<bool>,
    pub(super) attempt_elapsed: Duration,
}

pub(super) struct AttemptErrorState<'a> {
    pub(super) tried_tools: &'a mut Vec<String>,
    pub(super) tried_specs: &'a mut Vec<String>,
    pub(super) runtime_fallback_candidates: &'a mut Vec<ToolName>,
    pub(super) runtime_fallback_attempts: &'a mut u8,
    pub(super) fallback_chain: &'a mut csa_scheduler::FallbackChain,
    pub(super) pre_created_fork_session_id: &'a mut Option<String>,
}

pub(super) fn handle_attempt_error(
    error: anyhow::Error,
    request: AttemptErrorRequest<'_>,
    state: AttemptErrorState<'_>,
) -> Result<AttemptErrorAction> {
    let AttemptErrorState {
        tried_tools,
        tried_specs,
        runtime_fallback_candidates,
        runtime_fallback_attempts,
        fallback_chain,
        pre_created_fork_session_id,
    } = state;

    if let Some(timeout_secs) = run_error_timeout_seconds(&error, request.run_timeout_seconds) {
        let interrupted_session_id = extract_meta_session_id_from_error(&error)
            .or_else(|| request.executed_session_id.map(str::to_owned))
            .or_else(|| pre_created_fork_session_id.clone())
            .or_else(|| request.effective_session_arg.map(str::to_owned));
        persist_fork_timeout_result_if_missing(
            request.project_root,
            request.is_fork,
            request.current_tool,
            interrupted_session_id.as_deref(),
            chrono::Utc::now(),
            timeout_secs,
        );
        let exit_code = emit_run_timeout(
            request.output_format,
            timeout_secs,
            request.current_tool,
            request.skill,
            interrupted_session_id.as_deref(),
        )?;
        return Ok(AttemptErrorAction::Exit(exit_code));
    }

    if let Some(signal_exit_code) = signal_interruption_exit_code(&error) {
        cleanup_pre_created_fork_session(pre_created_fork_session_id, request.project_root);
        let interrupted_session_id = extract_meta_session_id_from_error(&error)
            .or_else(|| request.executed_session_id.map(str::to_owned))
            .or_else(|| request.effective_session_arg.map(str::to_owned));
        let signal_name = signal_name_from_exit_code(signal_exit_code);

        match request.output_format {
            OutputFormat::Text => {
                if let Some(ref session_id) = interrupted_session_id {
                    let resume_hint =
                        build_resume_hint_command(session_id, request.current_tool, request.skill);
                    eprintln!(
                        "csa run interrupted by {signal_name} (exit {signal_exit_code}). Resume with:\n  {resume_hint}"
                    );
                } else {
                    eprintln!(
                        "csa run interrupted by {signal_name} (exit {signal_exit_code}). Resume by reusing the interrupted session with `csa run --session <session-id> ...`."
                    );
                }
            }
            OutputFormat::Json => {
                let resume_hint = interrupted_session_id.as_ref().map(|session_id| {
                    build_resume_hint_command(session_id, request.current_tool, request.skill)
                });
                let json_error = serde_json::json!({
                    "error": "interrupted",
                    "signal": signal_name,
                    "exit_code": signal_exit_code,
                    "session_id": interrupted_session_id,
                    "resume_hint": resume_hint,
                    "message": error.to_string()
                });
                println!("{}", serde_json::to_string_pretty(&json_error)?);
            }
        }

        return Ok(AttemptErrorAction::Exit(signal_exit_code));
    }

    let full_error_chain = format!("{error:#}");
    let permanent_tool_exhaustion = is_permanent_tool_exhaustion_error(
        request.tool_name,
        &full_error_chain,
        request.current_model_spec,
    );
    if request.runtime_fallback_enabled
        && *runtime_fallback_attempts < request.max_runtime_fallback_attempts
        && !permanent_tool_exhaustion
        && let Some(next_tool) = take_next_runtime_fallback_tool(
            runtime_fallback_candidates,
            request.current_tool,
            tried_tools,
        )
    {
        *runtime_fallback_attempts += 1;
        warn!(
            from = %request.tool_name,
            to = %next_tool.as_str(),
            attempt = *runtime_fallback_attempts,
            max_attempts = request.max_runtime_fallback_attempts,
            error = %error,
            "HeterogeneousPreferred runtime fallback: retrying with next heterogeneous tool"
        );
        tried_tools.push(request.tool_name.to_string());
        cleanup_pre_created_fork_session(pre_created_fork_session_id, request.project_root);
        return Ok(AttemptErrorAction::Retry(AttemptRetryAction::Retry {
            new_tool: next_tool,
            new_model_spec: None,
            failover_context: FailoverContextUpdate::Preserve,
        }));
    }

    match evaluate_error_rate_limit_failover_with_global_config(ErrorRateLimitFailoverRequest {
        tool_name_str: request.tool_name,
        error_message: &full_error_chain,
        attempts: request.attempts,
        max_failover_attempts: request.max_failover_attempts,
        tried_tools,
        tried_specs,
        tier_auto_select: request.tier_auto_select,
        failover_on_crash_enabled: request.failover_on_crash_enabled,
        resolved_tier_name: request.resolved_tier_name,
        tier_failover_tool_filter: request.tier_failover_tool_filter,
        executed_session_id: request.executed_session_id,
        effective_session_arg: request.effective_session_arg,
        ephemeral: request.ephemeral,
        prompt_text: request.prompt_text,
        project_root: request.project_root,
        config: request.config,
        global_config: Some(request.global_config),
        task_needs_edit: request.task_needs_edit,
        current_model_spec: request.current_model_spec,
        fallback_chain,
        attempt_elapsed: Some(request.attempt_elapsed),
    })? {
        RateLimitAction::Retry {
            new_tool,
            new_model_spec,
        } => {
            let failover_context =
                build_failover_context_addendum(request.tool_name, request.executed_session_id);
            cleanup_pre_created_fork_session(pre_created_fork_session_id, request.project_root);
            Ok(AttemptErrorAction::Retry(AttemptRetryAction::Retry {
                new_tool,
                new_model_spec,
                failover_context: FailoverContextUpdate::Replace(failover_context),
            }))
        }
        RateLimitAction::ExhaustedFailovers { reason } => {
            cleanup_pre_created_fork_session(pre_created_fork_session_id, request.project_root);
            Ok(AttemptErrorAction::Error(
                error.context(format!("tier failover unavailable: {reason}")),
            ))
        }
        RateLimitAction::NoRateLimit => {
            cleanup_pre_created_fork_session(pre_created_fork_session_id, request.project_root);
            Ok(AttemptErrorAction::Error(error))
        }
    }
}

pub(super) enum PostAttemptAction {
    Retry(AttemptRetryAction),
    Break(Option<Vec<String>>),
}

pub(super) struct PostAttemptRequest<'a> {
    pub(super) exec_result: &'a mut csa_process::ExecutionResult,
    pub(super) exec_changed_paths: Option<Vec<String>>,
    pub(super) issue_tokens_used: u64,
    pub(super) runtime_fallback_enabled: bool,
    pub(super) max_runtime_fallback_attempts: u8,
    pub(super) current_tool: ToolName,
    pub(super) tool_name: &'a str,
    pub(super) current_model_spec: Option<&'a str>,
    pub(super) attempts: usize,
    pub(super) max_failover_attempts: usize,
    pub(super) tier_auto_select: bool,
    pub(super) resolved_tier_name: Option<&'a str>,
    pub(super) tier_failover_tool_filter: Option<ToolName>,
    pub(super) executed_session_id: Option<&'a str>,
    pub(super) effective_session_arg: Option<&'a str>,
    pub(super) ephemeral: bool,
    pub(super) prompt_text: &'a str,
    pub(super) project_root: &'a Path,
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) global_config: &'a GlobalConfig,
    pub(super) task_needs_edit: Option<bool>,
    pub(super) attempt_elapsed: Duration,
}

pub(super) struct PostAttemptState<'a> {
    pub(super) tried_tools: &'a mut Vec<String>,
    pub(super) tried_specs: &'a mut Vec<String>,
    pub(super) runtime_fallback_candidates: &'a mut Vec<ToolName>,
    pub(super) runtime_fallback_attempts: &'a mut u8,
    pub(super) fallback_chain: &'a mut csa_scheduler::FallbackChain,
    pub(super) accumulated_changed_paths: &'a mut Vec<String>,
    pub(super) all_attempt_change_snapshots_available: &'a mut bool,
    pub(super) pre_created_fork_session_id: &'a mut Option<String>,
}

pub(super) fn evaluate_post_attempt_retry(
    request: PostAttemptRequest<'_>,
    state: PostAttemptState<'_>,
) -> Result<PostAttemptAction> {
    let PostAttemptState {
        tried_tools,
        tried_specs,
        runtime_fallback_candidates,
        runtime_fallback_attempts,
        fallback_chain,
        accumulated_changed_paths,
        all_attempt_change_snapshots_available,
        pre_created_fork_session_id,
    } = state;

    let permanent_tool_exhaustion = detect_permanent_tool_exhaustion_result(
        request.tool_name,
        request.exec_result,
        request.current_model_spec,
    )
    .is_some();

    if request.exec_result.exit_code != 0
        && request
            .global_config
            .budget
            .is_exhausted(request.issue_tokens_used)
    {
        annotate_issue_budget_exhaustion(
            request.exec_result,
            request.issue_tokens_used,
            request.global_config.budget.resolved_max_tokens_per_issue(),
        );
        return Ok(PostAttemptAction::Break(request.exec_changed_paths));
    }

    if request.exec_result.exit_code != 0
        && request.runtime_fallback_enabled
        && *runtime_fallback_attempts < request.max_runtime_fallback_attempts
        && !is_post_run_commit_policy_gate_failure(request.exec_result)
        && !permanent_tool_exhaustion
        && let Some(next_tool) = take_next_runtime_fallback_tool(
            runtime_fallback_candidates,
            request.current_tool,
            tried_tools,
        )
    {
        merge_retry_changed_paths(
            accumulated_changed_paths,
            all_attempt_change_snapshots_available,
            request.exec_changed_paths,
        );
        *runtime_fallback_attempts += 1;
        warn!(
            from = %request.tool_name,
            to = %next_tool.as_str(),
            exit_code = request.exec_result.exit_code,
            attempt = *runtime_fallback_attempts,
            max_attempts = request.max_runtime_fallback_attempts,
            "HeterogeneousPreferred runtime fallback: retrying with next heterogeneous tool"
        );
        tried_tools.push(request.tool_name.to_string());
        cleanup_pre_created_fork_session(pre_created_fork_session_id, request.project_root);
        return Ok(PostAttemptAction::Retry(AttemptRetryAction::Retry {
            new_tool: next_tool,
            new_model_spec: None,
            failover_context: FailoverContextUpdate::Preserve,
        }));
    }

    if is_post_run_commit_policy_gate_failure(request.exec_result) {
        return Ok(PostAttemptAction::Break(request.exec_changed_paths));
    }

    match evaluate_rate_limit_failover_with_global_config(RateLimitFailoverRequest {
        tool_name_str: request.tool_name,
        exec_result: request.exec_result,
        attempts: request.attempts,
        max_failover_attempts: request.max_failover_attempts,
        tried_tools,
        tried_specs,
        tier_auto_select: request.tier_auto_select,
        resolved_tier_name: request.resolved_tier_name,
        tier_failover_tool_filter: request.tier_failover_tool_filter,
        executed_session_id: request.executed_session_id,
        effective_session_arg: request.effective_session_arg,
        ephemeral: request.ephemeral,
        prompt_text: request.prompt_text,
        project_root: request.project_root,
        config: request.config,
        global_config: Some(request.global_config),
        task_needs_edit: request.task_needs_edit,
        current_model_spec: request.current_model_spec,
        fallback_chain,
        attempt_elapsed: Some(request.attempt_elapsed),
    })? {
        RateLimitAction::Retry {
            new_tool,
            new_model_spec,
        } => {
            merge_retry_changed_paths(
                accumulated_changed_paths,
                all_attempt_change_snapshots_available,
                request.exec_changed_paths,
            );
            let failover_context =
                build_failover_context_addendum(request.tool_name, request.executed_session_id);
            cleanup_pre_created_fork_session(pre_created_fork_session_id, request.project_root);
            Ok(PostAttemptAction::Retry(AttemptRetryAction::Retry {
                new_tool,
                new_model_spec,
                failover_context: FailoverContextUpdate::Replace(failover_context),
            }))
        }
        RateLimitAction::ExhaustedFailovers { reason } => {
            annotate_failover_exhaustion(request.exec_result, &reason);
            Ok(PostAttemptAction::Break(request.exec_changed_paths))
        }
        RateLimitAction::NoRateLimit => {
            if request.exec_result.exit_code != 0 && request.tier_auto_select {
                warn!(
                    tool = %request.tool_name,
                    exit_code = request.exec_result.exit_code,
                    summary = %request.exec_result.summary,
                    "Run failed but not classified as rate-limit/failover-eligible; no tier fallback attempted"
                );
            }
            Ok(PostAttemptAction::Break(request.exec_changed_paths))
        }
    }
}

fn annotate_failover_exhaustion(exec_result: &mut csa_process::ExecutionResult, reason: &str) {
    let detail = format!("tier failover unavailable: {reason}");
    if !exec_result.summary.contains(&detail) {
        if exec_result.summary.trim().is_empty() || exec_result.summary.starts_with("exit code ") {
            exec_result.summary = detail.clone();
        } else {
            exec_result.summary = format!("{}; {detail}", exec_result.summary);
        }
    }
    if !exec_result.stderr_output.contains(&detail) {
        if !exec_result.stderr_output.is_empty() && !exec_result.stderr_output.ends_with('\n') {
            exec_result.stderr_output.push('\n');
        }
        exec_result.stderr_output.push_str(&detail);
        exec_result.stderr_output.push('\n');
    }
}

fn annotate_issue_budget_exhaustion(
    exec_result: &mut csa_process::ExecutionResult,
    used_tokens: u64,
    max_tokens: u64,
) {
    let message = format!(
        "Issue token budget exhausted; stopping retry loop (used={used_tokens}, max={max_tokens})."
    );
    exec_result.warnings.push(message.clone());
    if !exec_result.stderr_output.ends_with('\n') && !exec_result.stderr_output.is_empty() {
        exec_result.stderr_output.push('\n');
    }
    exec_result.stderr_output.push_str(&message);
    exec_result.stderr_output.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_budget_exhaustion_stops_retry_before_runtime_fallback() {
        let project_root = tempfile::tempdir().expect("project root");
        let mut result = csa_process::ExecutionResult {
            exit_code: 1,
            summary: "failed".to_string(),
            ..Default::default()
        };
        let mut global_config = GlobalConfig::default();
        global_config.budget.max_tokens_per_issue = 10;

        let mut tried_tools = Vec::new();
        let mut tried_specs = Vec::new();
        let mut runtime_fallback_candidates = vec![ToolName::ClaudeCode];
        let mut runtime_fallback_attempts = 0;
        let mut fallback_chain: csa_scheduler::FallbackChain = Vec::new();
        let mut accumulated_changed_paths = Vec::new();
        let mut all_attempt_change_snapshots_available = true;
        let mut pre_created_fork_session_id = None;

        let action = evaluate_post_attempt_retry(
            PostAttemptRequest {
                exec_result: &mut result,
                exec_changed_paths: None,
                issue_tokens_used: 10,
                runtime_fallback_enabled: true,
                max_runtime_fallback_attempts: 3,
                current_tool: ToolName::Codex,
                tool_name: ToolName::Codex.as_str(),
                current_model_spec: None,
                attempts: 1,
                max_failover_attempts: 3,
                tier_auto_select: false,
                resolved_tier_name: None,
                tier_failover_tool_filter: None,
                executed_session_id: None,
                effective_session_arg: None,
                ephemeral: false,
                prompt_text: "do work",
                project_root: project_root.path(),
                config: None,
                global_config: &global_config,
                task_needs_edit: None,
                attempt_elapsed: Duration::ZERO,
            },
            PostAttemptState {
                tried_tools: &mut tried_tools,
                tried_specs: &mut tried_specs,
                runtime_fallback_candidates: &mut runtime_fallback_candidates,
                runtime_fallback_attempts: &mut runtime_fallback_attempts,
                fallback_chain: &mut fallback_chain,
                accumulated_changed_paths: &mut accumulated_changed_paths,
                all_attempt_change_snapshots_available: &mut all_attempt_change_snapshots_available,
                pre_created_fork_session_id: &mut pre_created_fork_session_id,
            },
        )
        .expect("post-attempt evaluation should succeed");

        assert!(matches!(action, PostAttemptAction::Break(None)));
        assert_eq!(runtime_fallback_attempts, 0);
        assert!(tried_tools.is_empty());
        assert!(
            result.warnings.iter().any(
                |warning| warning.contains("Issue token budget exhausted; stopping retry loop")
            )
        );
        assert!(
            result
                .stderr_output
                .contains("Issue token budget exhausted; stopping retry loop (used=10, max=10).")
        );
    }
}
