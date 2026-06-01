use anyhow::Result;
use csa_config::{ExecutionEnvOptions, GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolSelectionStrategy};
use csa_executor::structured_output_instructions_for_fork_call;
use csa_lock::slot::{
    SlotAcquireResult, ToolSlot, acquire_slot_blocking, format_slot_diagnostic, slot_usage,
    try_acquire_slot,
};
use std::time::Instant;
use tracing::{info, warn};

use super::attempt_exec::{
    AttemptExecution, EphemeralRunRequest, run_ephemeral_with_timeout,
    run_ephemeral_without_timeout, run_persistent_with_timeout, run_persistent_without_timeout,
};
use super::attempt_support::{
    allow_cross_tool_failover, build_failover_context_addendum, merge_retry_changed_paths,
    merge_run_loop_changed_paths, persist_fork_timeout_result_if_missing,
    resolve_attempt_initial_response_timeout_seconds,
};
use super::policy::is_post_run_commit_policy_block;
use super::resume::{
    build_resume_hint_command, emit_run_timeout, extract_meta_session_id_from_error,
    resolve_remaining_run_timeout, run_error_timeout_seconds, signal_interruption_exit_code,
    signal_name_from_exit_code,
};
use crate::pipeline;
use crate::run_cmd_fork::{
    ForkResolution, cleanup_pre_created_fork_session, pre_create_native_fork_session, resolve_fork,
};
use crate::run_cmd_post::{
    RateLimitAction, detect_permanent_tool_exhaustion_result, evaluate_error_rate_limit_failover,
    evaluate_rate_limit_failover, is_permanent_tool_exhaustion_error,
};
use crate::run_cmd_tool_selection::{
    resolve_slot_wait_timeout_seconds, take_next_runtime_fallback_tool,
};
use crate::run_helpers::{is_tool_binary_available_for_config, parse_tool_name};

#[path = "run_cmd_attempt_types.rs"]
mod types;
pub(crate) use types::{RunLoopCompletion, RunLoopOutcome, RunLoopRequest};

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
        let mut _slot_guard: Option<ToolSlot>;

        match try_acquire_slot(
            &slots_dir,
            tool_name_str,
            max_concurrent,
            session_arg.as_deref(),
        )? {
            SlotAcquireResult::Acquired(slot) => {
                info!(
                    tool = %tool_name_str,
                    slot = slot.slot_index(),
                    max = max_concurrent,
                    "Acquired global slot"
                );
                _slot_guard = Some(slot);
            }
            SlotAcquireResult::Exhausted(status) => {
                let all_tools = request.global_config.all_tool_slots();
                let all_tools_ref: Vec<(&str, u32)> =
                    all_tools.iter().map(|(n, m)| (*n, *m)).collect();
                let all_usage = slot_usage(&slots_dir, &all_tools_ref);
                let diag_msg = format_slot_diagnostic(tool_name_str, &status, &all_usage);

                if cross_tool_failover_enabled && attempts < max_failover_attempts {
                    let free_alt = all_usage.iter().find(|s| {
                        s.tool_name != tool_name_str
                            && s.free() > 0
                            && !tried_tools.contains(&s.tool_name)
                            && request
                                .config
                                .map(|c| c.is_tool_auto_selectable(&s.tool_name))
                                .unwrap_or(false)
                            && is_tool_binary_available_for_config(&s.tool_name, request.config)
                    });

                    if let Some(alt) = free_alt {
                        info!(
                            from = %tool_name_str,
                            to = %alt.tool_name,
                            reason = "slot_exhausted",
                            "Failing over to tool with free slots"
                        );
                        tried_tools.push(tool_name_str.to_string());
                        current_tool = parse_tool_name(&alt.tool_name)?;
                        current_model_spec = None;
                        current_model = None;
                        fork_resolution = None;
                        if is_fork {
                            effective_session_arg = None;
                        }
                        continue;
                    }
                }

                if request.wait {
                    info!(
                        tool = %tool_name_str,
                        "All slots occupied, waiting for a free slot"
                    );
                    let timeout = std::time::Duration::from_secs(
                        resolve_slot_wait_timeout_seconds(request.config),
                    );
                    let slot = acquire_slot_blocking(
                        &slots_dir,
                        tool_name_str,
                        max_concurrent,
                        timeout,
                        session_arg.as_deref(),
                    )?;
                    info!(
                        tool = %tool_name_str,
                        slot = slot.slot_index(),
                        "Acquired slot after waiting"
                    );
                    _slot_guard = Some(slot);
                } else {
                    eprintln!("{diag_msg}");
                    if matches!(request.strategy, ToolSelectionStrategy::Explicit(_))
                        && !cross_tool_failover_enabled
                    {
                        eprintln!(
                            "Explicit --tool {tool_name_str} is currently unavailable. Retry later or choose a different --tool."
                        );
                    }
                    return Ok(RunLoopCompletion::Exit(1));
                }
            }
        }

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

        let mut extra_env = request.global_config.build_execution_env(
            tool_name_str,
            ExecutionEnvOptions::from_no_failover(request.no_failover),
        );
        crate::executor_csa_guard::mark_skill_executor_env(&mut extra_env, request.skill.is_some());
        // #1741 (canonical pin-CONSUMING resolution): csa run resolved a subtree
        // pin (explicit or inherited). It is carried OUT-OF-BAND from `extra_env`
        // as a typed `SubtreeModelPin` and applied by the executor's trusted
        // channel after the generic env merge — never via the env map, so no
        // user/request/config env can spoof it. review/debate mirror this with
        // their own resolved spec; batch/plan/claude-sub-agent (which do NOT
        // consume the pin) cascade an inherited pin via
        // `inherited_subtree_model_pin`. Self-gated on the pin.
        let subtree_pin = crate::run_cmd_model_pin::resolve_subtree_model_pin(
            request.subtree_model_pin_spec,
            request.force_ignore_tier_setting,
            request.no_failover,
        );
        let mut effective_prompt = if let Some(ref fork_res) = fork_resolution {
            if let Some(ref ctx) = fork_res.context_prefix {
                info!(
                    context_len = ctx.len(),
                    "Prepending soft fork context to prompt"
                );
                format!("{ctx}\n\n---\n\n{}", request.prompt_text)
            } else {
                request.prompt_text.to_string()
            }
        } else {
            request.prompt_text.to_string()
        };

        // Prepend context recovery instructions for rate-limit failover retries.
        if let Some(ref addendum) = failover_context_addendum {
            effective_prompt = format!("{addendum}\n\n---\n\n{effective_prompt}");
        }
        if let Some(guard) = crate::run_cmd_model_pin::subtree_model_pin_prompt_guard(
            request.subtree_model_pin_spec,
            request.force_ignore_tier_setting,
            request.no_failover,
        ) {
            effective_prompt = format!("{guard}\n\n{effective_prompt}");
        }

        if request.fork_call
            && let Some(instructions) = structured_output_instructions_for_fork_call(true)
        {
            effective_prompt.push_str(instructions);
        }
        if let Some(guard) = crate::pipeline::prompt_guard::anti_recursion_guard(request.config) {
            effective_prompt = format!("{guard}\n\n{effective_prompt}");
        }
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
                    request.resolved_tier_name,
                    request.context_load_options,
                    request.stream_mode,
                    request.idle_timeout_seconds,
                    initial_response_timeout_seconds,
                    timeout_duration,
                    &request.memory_injection,
                    request.global_config,
                    request.pre_session_hook.clone(),
                    fork_resolution.as_ref(),
                    fresh_spawn_preflight_override,
                    &mut executed_session_id,
                    &mut pre_created_fork_session_id,
                    request.no_fs_sandbox,
                    &request.extra_writable,
                    &request.extra_readable,
                    request.no_error_marker_scan,
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
                request.resolved_tier_name,
                request.context_load_options,
                request.stream_mode,
                request.idle_timeout_seconds,
                initial_response_timeout_seconds,
                &request.memory_injection,
                request.global_config,
                request.pre_session_hook.clone(),
                fork_resolution.as_ref(),
                fresh_spawn_preflight_override,
                &mut executed_session_id,
                &mut pre_created_fork_session_id,
                request.no_fs_sandbox,
                &request.extra_writable,
                &request.extra_readable,
                request.no_error_marker_scan,
            )
            .await
        }?;

        let (exec_result, exec_changed_paths) = match attempt_execution {
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
                result: Ok(result),
                changed_paths: attempt_changed_paths,
            } => (result, attempt_changed_paths),
            AttemptExecution::Finished {
                result: Err(e),
                changed_paths: _,
            } => {
                if let Some(timeout_secs) =
                    run_error_timeout_seconds(&e, request.run_timeout_seconds)
                {
                    let interrupted_session_id = extract_meta_session_id_from_error(&e)
                        .or_else(|| executed_session_id.clone())
                        .or_else(|| pre_created_fork_session_id.clone())
                        .or_else(|| effective_session_arg.clone());
                    persist_fork_timeout_result_if_missing(
                        request.project_root,
                        is_fork,
                        current_tool,
                        interrupted_session_id.as_deref(),
                        chrono::Utc::now(),
                        timeout_secs,
                    );
                    let exit_code = emit_run_timeout(
                        request.output_format,
                        timeout_secs,
                        current_tool,
                        request.skill,
                        interrupted_session_id.as_deref(),
                    )?;
                    return Ok(RunLoopCompletion::Exit(exit_code));
                }
                if let Some(signal_exit_code) = signal_interruption_exit_code(&e) {
                    cleanup_pre_created_fork_session(
                        &mut pre_created_fork_session_id,
                        request.project_root,
                    );
                    let interrupted_session_id = extract_meta_session_id_from_error(&e)
                        .or_else(|| executed_session_id.clone())
                        .or_else(|| effective_session_arg.clone());
                    let signal_name = signal_name_from_exit_code(signal_exit_code);

                    match request.output_format {
                        OutputFormat::Text => {
                            if let Some(ref session_id) = interrupted_session_id {
                                let resume_hint = build_resume_hint_command(
                                    session_id,
                                    current_tool,
                                    request.skill,
                                );
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
                                build_resume_hint_command(session_id, current_tool, request.skill)
                            });
                            let json_error = serde_json::json!({
                                "error": "interrupted",
                                "signal": signal_name,
                                "exit_code": signal_exit_code,
                                "session_id": interrupted_session_id,
                                "resume_hint": resume_hint,
                                "message": e.to_string()
                            });
                            println!("{}", serde_json::to_string_pretty(&json_error)?);
                        }
                    }

                    return Ok(RunLoopCompletion::Exit(signal_exit_code));
                }
                let full_error_chain = format!("{e:#}");
                let permanent_tool_exhaustion = is_permanent_tool_exhaustion_error(
                    tool_name_str,
                    &full_error_chain,
                    current_model_spec.as_deref(),
                );
                if runtime_fallback_enabled
                    && runtime_fallback_attempts < max_runtime_fallback_attempts
                    && !permanent_tool_exhaustion
                    && let Some(next_tool) = take_next_runtime_fallback_tool(
                        &mut runtime_fallback_candidates,
                        current_tool,
                        &tried_tools,
                    )
                {
                    runtime_fallback_attempts += 1;
                    warn!(
                        from = %tool_name_str,
                        to = %next_tool.as_str(),
                        attempt = runtime_fallback_attempts,
                        max_attempts = max_runtime_fallback_attempts,
                        error = %e,
                        "HeterogeneousPreferred runtime fallback: retrying with next heterogeneous tool"
                    );
                    tried_tools.push(tool_name_str.to_string());
                    current_tool = next_tool;
                    current_model_spec = None;
                    current_model = None;
                    fork_resolution = None;
                    if is_fork {
                        effective_session_arg = None;
                    }
                    cleanup_pre_created_fork_session(
                        &mut pre_created_fork_session_id,
                        request.project_root,
                    );
                    continue;
                }
                // ACP transport errors can bury the root cause under anyhow
                // context layers. Preserve the full chain so quota/crash
                // markers survive error-path failover detection.
                match evaluate_error_rate_limit_failover(
                    tool_name_str,
                    &full_error_chain,
                    attempts,
                    max_failover_attempts,
                    &mut tried_tools,
                    &mut tried_specs,
                    request.tier_auto_select,
                    request.failover_on_crash_enabled,
                    request.resolved_tier_name,
                    executed_session_id.as_deref(),
                    effective_session_arg.as_deref(),
                    request.ephemeral,
                    request.prompt_text,
                    request.project_root,
                    request.config,
                    request.task_needs_edit,
                    current_model_spec.as_deref(),
                    &mut fallback_chain,
                    Some(attempt_started_at.elapsed()),
                )? {
                    RateLimitAction::Retry {
                        new_tool,
                        new_model_spec,
                    } => {
                        failover_context_addendum = build_failover_context_addendum(
                            tool_name_str,
                            executed_session_id.as_deref(),
                        );
                        current_tool = new_tool;
                        current_model_spec = new_model_spec;
                        current_model = None;
                        fork_resolution = None;
                        if is_fork {
                            effective_session_arg = None;
                        }
                        cleanup_pre_created_fork_session(
                            &mut pre_created_fork_session_id,
                            request.project_root,
                        );
                        continue;
                    }
                    _ => {
                        cleanup_pre_created_fork_session(
                            &mut pre_created_fork_session_id,
                            request.project_root,
                        );
                        return Err(e);
                    }
                }
            }
        };

        let permanent_tool_exhaustion = detect_permanent_tool_exhaustion_result(
            tool_name_str,
            &exec_result,
            current_model_spec.as_deref(),
        )
        .is_some();

        if exec_result.exit_code != 0
            && runtime_fallback_enabled
            && runtime_fallback_attempts < max_runtime_fallback_attempts
            && !is_post_run_commit_policy_block(&exec_result.summary)
            && !permanent_tool_exhaustion
            && let Some(next_tool) = take_next_runtime_fallback_tool(
                &mut runtime_fallback_candidates,
                current_tool,
                &tried_tools,
            )
        {
            merge_retry_changed_paths(
                &mut accumulated_changed_paths,
                &mut all_attempt_change_snapshots_available,
                exec_changed_paths,
            );
            runtime_fallback_attempts += 1;
            warn!(
                from = %tool_name_str,
                to = %next_tool.as_str(),
                exit_code = exec_result.exit_code,
                attempt = runtime_fallback_attempts,
                max_attempts = max_runtime_fallback_attempts,
                "HeterogeneousPreferred runtime fallback: retrying with next heterogeneous tool"
            );
            tried_tools.push(tool_name_str.to_string());
            current_tool = next_tool;
            current_model_spec = None;
            current_model = None;
            fork_resolution = None;
            if is_fork {
                effective_session_arg = None;
            }
            cleanup_pre_created_fork_session(
                &mut pre_created_fork_session_id,
                request.project_root,
            );
            continue;
        }

        if is_post_run_commit_policy_block(&exec_result.summary) {
            break (exec_result, exec_changed_paths);
        }

        match evaluate_rate_limit_failover(
            tool_name_str,
            &exec_result,
            attempts,
            max_failover_attempts,
            &mut tried_tools,
            &mut tried_specs,
            request.tier_auto_select,
            request.resolved_tier_name,
            executed_session_id.as_deref(),
            effective_session_arg.as_deref(),
            request.ephemeral,
            request.prompt_text,
            request.project_root,
            request.config,
            request.task_needs_edit,
            current_model_spec.as_deref(),
            &mut fallback_chain,
            Some(attempt_started_at.elapsed()),
        )? {
            RateLimitAction::Retry {
                new_tool,
                new_model_spec,
            } => {
                merge_retry_changed_paths(
                    &mut accumulated_changed_paths,
                    &mut all_attempt_change_snapshots_available,
                    exec_changed_paths,
                );
                // Build xurl context recovery addendum so the failover
                // tool can retrieve the original session's conversation.
                failover_context_addendum =
                    build_failover_context_addendum(tool_name_str, executed_session_id.as_deref());
                current_tool = new_tool;
                current_model_spec = new_model_spec;
                current_model = None;
                fork_resolution = None;
                if is_fork {
                    effective_session_arg = None;
                }
                cleanup_pre_created_fork_session(
                    &mut pre_created_fork_session_id,
                    request.project_root,
                );
                continue;
            }
            _ => break (exec_result, exec_changed_paths),
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
#[path = "run_cmd_attempt_http_failover_tests.rs"]
mod http_failover_tests;
#[cfg(test)]
#[path = "run_cmd_attempt_tests.rs"]
mod tests;
