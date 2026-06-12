use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolSelectionStrategy;
use std::time::Instant;
use tracing::warn;

use super::attempt_exec::{
    AttemptExecution, EphemeralRunRequest, run_ephemeral_with_timeout,
    run_ephemeral_without_timeout, run_persistent_with_timeout, run_persistent_without_timeout,
};
use super::attempt_support::{
    allow_cross_tool_failover, merge_run_loop_changed_paths,
    persist_fork_timeout_result_if_missing, resolve_attempt_initial_response_timeout_seconds,
};
use super::resume::{emit_run_timeout, resolve_remaining_run_timeout};
use crate::pipeline;
use crate::run_cmd_fork::{ForkResolution, pre_create_native_fork_session, resolve_fork};
use crate::run_cmd_tool_selection::resolve_slot_wait_timeout_seconds;

#[path = "run_cmd_attempt_types.rs"]
mod types;
pub(crate) use types::{RunLoopCompletion, RunLoopOutcome, RunLoopRequest};
#[path = "run_cmd_attempt_outcome.rs"]
mod outcome;
use outcome::{
    AttemptErrorAction, AttemptErrorRequest, AttemptErrorState, AttemptRetryState,
    PostAttemptAction, PostAttemptRequest, PostAttemptState, evaluate_post_attempt_retry,
    handle_attempt_error,
};
#[path = "run_cmd_attempt_slot.rs"]
mod slot;
use slot::{AttemptSlotOutcome, AttemptSlotRequest, acquire_attempt_slot};
#[path = "run_cmd_attempt_prompt.rs"]
mod prompt;
#[cfg(test)]
use prompt::resolve_attempt_subtree_model_pin_spec;
use prompt::{AttemptPromptRequest, build_attempt_prompt};

pub(crate) async fn execute_run_loop(request: RunLoopRequest<'_>) -> Result<RunLoopCompletion> {
    // Compute max failover attempts: count total models across ALL tiers to
    // allow cross-tier failover when the primary tier is exhausted (#493).
    let max_failover_attempts = if request.no_failover {
        1
    } else {
        request
            .config
            .map(|cfg| {
                cfg.tiers
                    .values()
                    .map(|t| t.models.len())
                    .sum::<usize>()
                    .max(1)
            })
            .unwrap_or(1)
    };

    let slots_dir = GlobalConfig::slots_dir()?;
    let mut current_tool = request.initial_tool;
    let mut current_model_spec = request.initial_model_spec;
    let mut current_model = request.initial_model;
    let mut tried_tools: Vec<String> = Vec::new();
    let mut tried_specs: Vec<String> = Vec::new();
    let mut fallback_chain: csa_scheduler::FallbackChain = Vec::new();
    let mut attempts = 0;
    let runtime_fallback_enabled = matches!(
        request.strategy,
        ToolSelectionStrategy::HeterogeneousPreferred
    ) && !request.no_failover;
    let mut runtime_fallback_candidates = request.runtime_fallback_candidates;
    let mut runtime_fallback_attempts = 0u8;
    let max_runtime_fallback_attempts = 1u8;
    let cross_tool_failover_enabled = allow_cross_tool_failover(
        request.strategy.clone(),
        request.resolved_tier_name,
        request.force_ignore_tier_setting,
        request.no_failover,
    );
    let mut executed_session_id: Option<String> = None;
    let mut pre_created_fork_session_id: Option<String> = None;
    let mut fork_resolution: Option<ForkResolution> = request.caller_fork_resolution;
    let mut is_fork = request.is_fork;
    let mut failover_context_addendum: Option<String> = None;
    let mut is_auto_seed_fork = request.is_auto_seed_fork;
    let mut session_arg = request.session_arg;
    let mut effective_session_arg = request.effective_session_arg;
    let mut vcs_probe_cache = crate::run_helpers_branch_guard::VcsProbeCache::default();
    let enforce_tier =
        !request.force && !request.force_ignore_tier_setting && !request.user_model_spec_explicit;
    let mut accumulated_changed_paths: Vec<String> = Vec::new();
    let mut all_attempt_change_snapshots_available = true;
    let (result, changed_paths) = loop {
        attempts += 1;
        let mut fresh_spawn_preflight_override = false;

        let mut executor = pipeline::build_and_validate_executor(
            &current_tool,
            current_model_spec.as_deref(),
            current_model.as_deref(),
            request.thinking,
            pipeline::ConfigRefs {
                project: request.config,
                global: Some(request.global_config),
            },
            enforce_tier,
            request.force_override_user_config,
            matches!(request.strategy, ToolSelectionStrategy::Explicit(_)),
        )
        .await?;
        let fast_mode = request.fast_but_more_cost
            || request
                .config
                .and_then(|c| c.tools.get("codex"))
                .and_then(|t| t.fast_mode)
                .unwrap_or(false)
            || request
                .global_config
                .tools
                .get("codex")
                .and_then(|t| t.fast_mode)
                .unwrap_or(false);
        if fast_mode {
            executor.enable_codex_fast_mode();
        }

        let tool_name_str = executor.tool_name();
        let initial_response_timeout_seconds = resolve_attempt_initial_response_timeout_seconds(
            request.config,
            request.cli_initial_response_timeout,
            request.cli_idle_timeout,
            request.no_idle_timeout,
            tool_name_str,
        );
        let max_concurrent = request.global_config.max_concurrent(tool_name_str);
        let mut _slot_guard = match acquire_attempt_slot(
            AttemptSlotRequest {
                slots_dir: &slots_dir,
                tool_name: tool_name_str,
                max_concurrent,
                session_arg: session_arg.as_deref(),
                global_config: request.global_config,
                config: request.config,
                cross_tool_failover_enabled,
                attempts,
                max_failover_attempts,
                wait: request.wait,
                strategy: &request.strategy,
            },
            &mut tried_tools,
        )? {
            AttemptSlotOutcome::Acquired(slot) => Some(slot),
            AttemptSlotOutcome::RetryWithTool(next_tool) => {
                current_tool = next_tool;
                current_model_spec = None;
                current_model = None;
                fork_resolution = None;
                if is_fork {
                    effective_session_arg = None;
                }
                continue;
            }
            AttemptSlotOutcome::Exit(exit_code) => return Ok(RunLoopCompletion::Exit(exit_code)),
        };

        if request.fork_call {
            let slot_timeout = resolve_slot_wait_timeout_seconds(request.config);
            match crate::run_cmd_fork::fork_call_slot_handoff(
                &mut _slot_guard,
                &slots_dir,
                tool_name_str,
                max_concurrent,
                request.wait,
                slot_timeout,
                session_arg.as_deref(),
            ) {
                Ok(child_slot) => _slot_guard = Some(child_slot),
                Err(e) => {
                    eprintln!("{e}");
                    return Ok(RunLoopCompletion::Exit(1));
                }
            }
        }

        if is_fork && fork_resolution.is_none() {
            if let Some(ref source_id) = session_arg {
                let codex_auto_trust = request.config.is_some_and(ProjectConfig::codex_auto_trust);
                match resolve_fork(
                    source_id,
                    current_tool.as_str(),
                    request.project_root,
                    codex_auto_trust,
                )
                .await
                {
                    Ok(res) => fork_resolution = Some(res),
                    Err(e) if is_auto_seed_fork => {
                        warn!(
                            error = %e,
                            source = %source_id,
                            "Auto seed fork resolution failed, falling back to cold start"
                        );
                        is_auto_seed_fork = false;
                        is_fork = false;
                        session_arg = None;
                    }
                    Err(e) => return Err(e),
                }
            } else if !is_auto_seed_fork {
                anyhow::bail!("Fork requested but no source session resolved");
            }
        }

        let branch_state = crate::run_helpers_branch_guard::observe_branch_state_with_cache(
            request.project_root,
            request.config,
            Some(&mut vcs_probe_cache),
        );
        if let Some(exit_code) = crate::run_helpers_branch_guard::evaluate_and_emit_refusal(
            &request.branch_guard,
            branch_state,
        ) {
            return Ok(RunLoopCompletion::Exit(exit_code));
        }

        if let Some(ref fork_res) = fork_resolution {
            let (pre_id, new_eff) = pre_create_native_fork_session(
                request.project_root,
                fork_res,
                &current_tool,
                request.description.as_deref(),
                effective_session_arg,
            )?;
            if pre_id.is_some() {
                pre_created_fork_session_id = pre_id;
                fresh_spawn_preflight_override = true;
            }
            effective_session_arg = new_eff;
        }

        let attempt_prompt = build_attempt_prompt(AttemptPromptRequest {
            global_config: request.global_config,
            tool_name: tool_name_str,
            no_failover: request.no_failover,
            build_jobs: request.build_jobs,
            skill: request.skill,
            run_resolved_pin_spec: request.subtree_model_pin_spec,
            current_attempt_model_spec: current_model_spec.as_deref(),
            subtree_model_pin_force_ignore_tier_setting: request
                .subtree_model_pin_force_ignore_tier_setting,
            fork_resolution: fork_resolution.as_ref(),
            prompt_text: request.prompt_text,
            failover_context_addendum: failover_context_addendum.as_deref(),
            fork_call: request.fork_call,
            allow_git_push: request.allow_git_push,
            config: request.config,
            startup_env: request.startup_env,
        });
        let extra_env = attempt_prompt.extra_env;
        let subtree_pin = attempt_prompt.subtree_pin;
        let effective_prompt = attempt_prompt.effective_prompt;
        let remaining_run_timeout =
            resolve_remaining_run_timeout(request.run_timeout_seconds, request.run_started_at);
        if remaining_run_timeout.is_some_and(|remaining| remaining.is_zero()) {
            let timeout_resume_session = executed_session_id
                .clone()
                .or_else(|| pre_created_fork_session_id.clone())
                .or_else(|| effective_session_arg.clone());
            persist_fork_timeout_result_if_missing(
                request.project_root,
                is_fork,
                current_tool,
                timeout_resume_session.as_deref(),
                chrono::Utc::now(),
                request
                    .run_timeout_seconds
                    .expect("run timeout should be present"),
            );
            let exit_code = emit_run_timeout(
                request.output_format,
                request
                    .run_timeout_seconds
                    .expect("run timeout should be present"),
                current_tool,
                request.skill,
                timeout_resume_session.as_deref(),
            )?;
            return Ok(RunLoopCompletion::Exit(exit_code));
        }

        let attempt_started_at = Instant::now();
        let attempt_execution = if let Some(timeout_duration) = remaining_run_timeout {
            if request.ephemeral {
                run_ephemeral_with_timeout(
                    EphemeralRunRequest {
                        executor: &executor,
                        effective_prompt: &effective_prompt,
                        project_root: request.project_root,
                        extra_env: extra_env.as_ref(),
                        subtree_pin: subtree_pin.as_ref(),
                        allow_git_push: request.allow_git_push,
                        stream_mode: request.stream_mode,
                        idle_timeout_seconds: request.idle_timeout_seconds,
                        initial_response_timeout_seconds,
                    },
                    timeout_duration,
                )
                .await
            } else {
                run_persistent_with_timeout(
                    &executor,
                    &current_tool,
                    &effective_prompt,
                    request.output_format,
                    effective_session_arg.clone(),
                    request.description.clone(),
                    request.skill_session_tag.clone(),
                    request.parent.clone(),
                    request.project_root,
                    request.config,
                    extra_env.as_ref(),
                    subtree_pin.as_ref(),
                    request.allow_git_push,
                    request.resolved_tier_name,
                    request.context_load_options,
                    request.stream_mode,
                    request.idle_timeout_seconds,
                    initial_response_timeout_seconds,
                    timeout_duration,
                    &request.memory_injection,
                    request.global_config,
                    request.pre_session_hook.clone(),
                    request.resource_overrides,
                    fork_resolution.as_ref(),
                    fresh_spawn_preflight_override,
                    &mut executed_session_id,
                    &mut pre_created_fork_session_id,
                    request.no_fs_sandbox,
                    &request.extra_writable,
                    &request.extra_readable,
                    request.error_marker_scan_override,
                    request.no_hook_bypass_scan,
                    request.startup_env,
                )
                .await
            }
        } else if request.ephemeral {
            run_ephemeral_without_timeout(EphemeralRunRequest {
                executor: &executor,
                effective_prompt: &effective_prompt,
                project_root: request.project_root,
                extra_env: extra_env.as_ref(),
                subtree_pin: subtree_pin.as_ref(),
                allow_git_push: request.allow_git_push,
                stream_mode: request.stream_mode,
                idle_timeout_seconds: request.idle_timeout_seconds,
                initial_response_timeout_seconds,
            })
            .await
        } else {
            run_persistent_without_timeout(
                &executor,
                &current_tool,
                &effective_prompt,
                request.output_format,
                effective_session_arg.clone(),
                request.description.clone(),
                request.skill_session_tag.clone(),
                request.parent.clone(),
                request.project_root,
                request.config,
                extra_env.as_ref(),
                subtree_pin.as_ref(),
                request.allow_git_push,
                request.resolved_tier_name,
                request.context_load_options,
                request.stream_mode,
                request.idle_timeout_seconds,
                initial_response_timeout_seconds,
                &request.memory_injection,
                request.global_config,
                request.pre_session_hook.clone(),
                request.resource_overrides,
                fork_resolution.as_ref(),
                fresh_spawn_preflight_override,
                &mut executed_session_id,
                &mut pre_created_fork_session_id,
                request.no_fs_sandbox,
                &request.extra_writable,
                &request.extra_readable,
                request.error_marker_scan_override,
                request.no_hook_bypass_scan,
                request.startup_env,
            )
            .await
        }?;

        let (mut exec_result, exec_changed_paths) = match attempt_execution {
            AttemptExecution::TimedOut => {
                let timeout_resume_session = executed_session_id
                    .clone()
                    .or_else(|| pre_created_fork_session_id.clone())
                    .or_else(|| effective_session_arg.clone());
                persist_fork_timeout_result_if_missing(
                    request.project_root,
                    is_fork,
                    current_tool,
                    timeout_resume_session.as_deref(),
                    chrono::Utc::now(),
                    request
                        .run_timeout_seconds
                        .expect("run timeout should be present"),
                );
                let exit_code = emit_run_timeout(
                    request.output_format,
                    request
                        .run_timeout_seconds
                        .expect("run timeout should be present"),
                    current_tool,
                    request.skill,
                    timeout_resume_session.as_deref(),
                )?;
                return Ok(RunLoopCompletion::Exit(exit_code));
            }
            AttemptExecution::Exit(exit_code) => return Ok(RunLoopCompletion::Exit(exit_code)),
            AttemptExecution::Finished {
                result,
                changed_paths: attempt_changed_paths,
            } => match *result {
                Ok(result) => (result, attempt_changed_paths),
                Err(e) => match handle_attempt_error(
                    e,
                    AttemptErrorRequest {
                        run_timeout_seconds: request.run_timeout_seconds,
                        project_root: request.project_root,
                        is_fork,
                        current_tool,
                        skill: request.skill,
                        output_format: request.output_format,
                        executed_session_id: executed_session_id.as_deref(),
                        effective_session_arg: effective_session_arg.as_deref(),
                        runtime_fallback_enabled,
                        max_runtime_fallback_attempts,
                        tool_name: tool_name_str,
                        current_model_spec: current_model_spec.as_deref(),
                        attempts,
                        max_failover_attempts,
                        tier_auto_select: request.tier_auto_select,
                        failover_on_crash_enabled: request.failover_on_crash_enabled,
                        resolved_tier_name: request.resolved_tier_name,
                        tier_failover_tool_filter: request.tier_failover_tool_filter,
                        ephemeral: request.ephemeral,
                        prompt_text: request.prompt_text,
                        config: request.config,
                        global_config: request.global_config,
                        task_needs_edit: request.task_needs_edit,
                        attempt_elapsed: attempt_started_at.elapsed(),
                    },
                    AttemptErrorState {
                        tried_tools: &mut tried_tools,
                        tried_specs: &mut tried_specs,
                        runtime_fallback_candidates: &mut runtime_fallback_candidates,
                        runtime_fallback_attempts: &mut runtime_fallback_attempts,
                        fallback_chain: &mut fallback_chain,
                        pre_created_fork_session_id: &mut pre_created_fork_session_id,
                    },
                )? {
                    AttemptErrorAction::Exit(exit_code) => {
                        return Ok(RunLoopCompletion::Exit(exit_code));
                    }
                    AttemptErrorAction::Error(error) => return Err(error),
                    AttemptErrorAction::Retry(action) => {
                        AttemptRetryState {
                            failover_context: &mut failover_context_addendum,
                            tool: &mut current_tool,
                            model_spec: &mut current_model_spec,
                            model: &mut current_model,
                            fork_resolution: &mut fork_resolution,
                            effective_session: &mut effective_session_arg,
                            is_fork,
                        }
                        .apply(action);
                        continue;
                    }
                },
            },
        };

        match evaluate_post_attempt_retry(
            PostAttemptRequest {
                exec_result: &mut exec_result,
                exec_changed_paths,
                runtime_fallback_enabled,
                max_runtime_fallback_attempts,
                current_tool,
                tool_name: tool_name_str,
                current_model_spec: current_model_spec.as_deref(),
                attempts,
                max_failover_attempts,
                tier_auto_select: request.tier_auto_select,
                resolved_tier_name: request.resolved_tier_name,
                tier_failover_tool_filter: request.tier_failover_tool_filter,
                executed_session_id: executed_session_id.as_deref(),
                effective_session_arg: effective_session_arg.as_deref(),
                ephemeral: request.ephemeral,
                prompt_text: request.prompt_text,
                project_root: request.project_root,
                config: request.config,
                global_config: request.global_config,
                task_needs_edit: request.task_needs_edit,
                attempt_elapsed: attempt_started_at.elapsed(),
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
        )? {
            PostAttemptAction::Retry(action) => {
                AttemptRetryState {
                    failover_context: &mut failover_context_addendum,
                    tool: &mut current_tool,
                    model_spec: &mut current_model_spec,
                    model: &mut current_model,
                    fork_resolution: &mut fork_resolution,
                    effective_session: &mut effective_session_arg,
                    is_fork,
                }
                .apply(action);
                continue;
            }
            PostAttemptAction::Break(changed_paths) => break (exec_result, changed_paths),
        }
    };
    let changed_paths = merge_run_loop_changed_paths(
        accumulated_changed_paths,
        all_attempt_change_snapshots_available,
        changed_paths,
    );
    Ok(RunLoopCompletion::Completed(Box::new(RunLoopOutcome {
        result,
        current_tool,
        executed_session_id,
        changed_paths,
        fork_resolution,
        fallback_chain,
    })))
}

#[cfg(test)]
#[path = "run_cmd_attempt_codex_quota_tests.rs"]
mod codex_quota_tests;
#[cfg(test)]
#[path = "run_cmd_attempt_git_push_tests.rs"]
mod git_push_tests;
#[cfg(test)]
#[path = "run_cmd_attempt_http_failover_tests.rs"]
mod http_failover_tests;
#[cfg(test)]
#[path = "run_cmd_attempt_tests.rs"]
mod tests;
