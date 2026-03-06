//! Execution loop for `csa run`.
//!
//! Extracted from `run_cmd.rs` to keep module sizes manageable.

use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use tracing::{info, warn};

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName, ToolSelectionStrategy};
use csa_executor::structured_output_instructions_for_fork_call;
use csa_lock::slot::{
    SlotAcquireResult, ToolSlot, acquire_slot_blocking, format_slot_diagnostic, slot_usage,
    try_acquire_slot,
};

use crate::pipeline;
use crate::run_cmd_fork::{
    ForkResolution, cleanup_pre_created_fork_session, pre_create_native_fork_session, resolve_fork,
};
use crate::run_cmd_post::{RateLimitAction, evaluate_rate_limit_failover};
use crate::run_cmd_tool_selection::{
    resolve_slot_wait_timeout_seconds, take_next_runtime_fallback_tool,
};
use crate::run_helpers::{is_tool_binary_available, parse_tool_name};

use super::attempt_exec::{
    AttemptExecution, run_ephemeral_with_timeout, run_ephemeral_without_timeout,
    run_persistent_with_timeout, run_persistent_without_timeout,
};
use super::policy::is_post_run_commit_policy_block;
use super::resume::{
    build_resume_hint_command, emit_run_timeout, extract_meta_session_id_from_error,
    resolve_remaining_run_timeout, signal_interruption_exit_code, signal_name_from_exit_code,
    wall_timeout_seconds_from_error,
};

pub(crate) struct RunLoopRequest<'a> {
    pub(crate) strategy: ToolSelectionStrategy,
    pub(crate) initial_tool: ToolName,
    pub(crate) initial_model_spec: Option<String>,
    pub(crate) initial_model: Option<String>,
    pub(crate) runtime_fallback_candidates: Vec<ToolName>,
    pub(crate) project_root: &'a Path,
    pub(crate) config: Option<&'a ProjectConfig>,
    pub(crate) global_config: &'a GlobalConfig,
    pub(crate) prompt_text: &'a str,
    pub(crate) skill: Option<&'a str>,
    pub(crate) skill_session_tag: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) parent: Option<String>,
    pub(crate) output_format: OutputFormat,
    pub(crate) stream_mode: csa_process::StreamMode,
    pub(crate) thinking: Option<&'a str>,
    pub(crate) force: bool,
    pub(crate) force_override_user_config: bool,
    pub(crate) no_failover: bool,
    pub(crate) wait: bool,
    pub(crate) idle_timeout_seconds: u64,
    pub(crate) run_timeout_seconds: Option<u64>,
    pub(crate) run_started_at: Instant,
    pub(crate) is_fork: bool,
    pub(crate) is_auto_seed_fork: bool,
    pub(crate) ephemeral: bool,
    pub(crate) fork_call: bool,
    pub(crate) session_arg: Option<String>,
    pub(crate) effective_session_arg: Option<String>,
    pub(crate) resolved_tier_name: Option<&'a str>,
    pub(crate) context_load_options: Option<&'a csa_executor::ContextLoadOptions>,
    pub(crate) memory_injection: pipeline::MemoryInjectionOptions,
}

pub(crate) enum RunLoopCompletion {
    Exit(i32),
    Completed(Box<RunLoopOutcome>),
}

pub(crate) struct RunLoopOutcome {
    pub(crate) result: csa_process::ExecutionResult,
    pub(crate) current_tool: ToolName,
    pub(crate) executed_session_id: Option<String>,
    pub(crate) fork_resolution: Option<ForkResolution>,
}

pub(crate) async fn execute_run_loop(request: RunLoopRequest<'_>) -> Result<RunLoopCompletion> {
    let max_failover_attempts = if request.no_failover {
        1
    } else {
        request
            .config
            .and_then(|cfg| {
                let tier_name = cfg
                    .tier_mapping
                    .get("default")
                    .cloned()
                    .unwrap_or_else(|| "tier3".to_string());
                cfg.tiers.get(&tier_name).map(|t| t.models.len())
            })
            .unwrap_or(1)
    };

    let slots_dir = GlobalConfig::slots_dir()?;
    let mut current_tool = request.initial_tool;
    let mut current_model_spec = request.initial_model_spec;
    let mut current_model = request.initial_model;
    let mut tried_tools: Vec<String> = Vec::new();
    let mut attempts = 0;
    let runtime_fallback_enabled = matches!(
        request.strategy,
        ToolSelectionStrategy::HeterogeneousPreferred
    ) && !request.no_failover;
    let mut runtime_fallback_candidates = request.runtime_fallback_candidates;
    let mut runtime_fallback_attempts = 0u8;
    let max_runtime_fallback_attempts = 1u8;
    let mut executed_session_id: Option<String> = None;
    let mut pre_created_fork_session_id: Option<String> = None;
    let mut fork_resolution: Option<ForkResolution> = None;
    let mut is_fork = request.is_fork;
    let mut is_auto_seed_fork = request.is_auto_seed_fork;
    let mut session_arg = request.session_arg;
    let mut effective_session_arg = request.effective_session_arg;

    let result = loop {
        attempts += 1;

        let executor = pipeline::build_and_validate_executor(
            &current_tool,
            current_model_spec.as_deref(),
            current_model.as_deref(),
            request.thinking,
            pipeline::ConfigRefs {
                project: request.config,
                global: Some(request.global_config),
            },
            !request.force,
            request.force_override_user_config,
        )
        .await?;

        let tool_name_str = executor.tool_name();
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

                if !request.no_failover && attempts < max_failover_attempts {
                    let free_alt = all_usage.iter().find(|s| {
                        s.tool_name != tool_name_str
                            && s.free() > 0
                            && !tried_tools.contains(&s.tool_name)
                            && request
                                .config
                                .map(|c| c.is_tool_auto_selectable(&s.tool_name))
                                .unwrap_or(false)
                            && is_tool_binary_available(&s.tool_name)
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
                    eprintln!("{}", diag_msg);
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
                    eprintln!("{}", e);
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
            }
            effective_session_arg = new_eff;
        }

        let extra_env = request.global_config.env_vars(tool_name_str).cloned();
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

        if request.fork_call
            && let Some(instructions) = structured_output_instructions_for_fork_call(true)
        {
            effective_prompt.push_str(instructions);
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
                request.is_fork,
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

        let attempt_execution = if let Some(timeout_duration) = remaining_run_timeout {
            if request.ephemeral {
                run_ephemeral_with_timeout(
                    &executor,
                    &effective_prompt,
                    extra_env.as_ref(),
                    request.stream_mode,
                    request.idle_timeout_seconds,
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
                    request.resolved_tier_name,
                    request.context_load_options,
                    request.stream_mode,
                    request.idle_timeout_seconds,
                    timeout_duration,
                    &request.memory_injection,
                    request.global_config,
                    fork_resolution.as_ref(),
                    &mut executed_session_id,
                    &mut pre_created_fork_session_id,
                )
                .await
            }
        } else if request.ephemeral {
            run_ephemeral_without_timeout(
                &executor,
                &effective_prompt,
                extra_env.as_ref(),
                request.stream_mode,
                request.idle_timeout_seconds,
            )
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
                request.resolved_tier_name,
                request.context_load_options,
                request.stream_mode,
                request.idle_timeout_seconds,
                &request.memory_injection,
                request.global_config,
                fork_resolution.as_ref(),
                &mut executed_session_id,
                &mut pre_created_fork_session_id,
            )
            .await
        }?;

        let exec_result = match attempt_execution {
            AttemptExecution::TimedOut => {
                let timeout_resume_session = executed_session_id
                    .clone()
                    .or_else(|| pre_created_fork_session_id.clone())
                    .or_else(|| effective_session_arg.clone());
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
            AttemptExecution::Finished(Ok(result)) => result,
            AttemptExecution::Finished(Err(e)) => {
                let wall_timeout_secs = wall_timeout_seconds_from_error(&e);
                if let Some(timeout_secs) = wall_timeout_secs.or(request.run_timeout_seconds) {
                    let interrupted_session_id = extract_meta_session_id_from_error(&e)
                        .or_else(|| executed_session_id.clone())
                        .or_else(|| pre_created_fork_session_id.clone())
                        .or_else(|| effective_session_arg.clone());
                    if wall_timeout_secs.is_some() {
                        persist_fork_timeout_result_if_missing(
                            request.project_root,
                            request.is_fork,
                            current_tool,
                            interrupted_session_id.as_deref(),
                            chrono::Utc::now(),
                            timeout_secs,
                        );
                    }
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
                                    "csa run interrupted by {} (exit {}). Resume with:\n  {}",
                                    signal_name, signal_exit_code, resume_hint
                                );
                            } else {
                                eprintln!(
                                    "csa run interrupted by {} (exit {}). Resume by reusing the interrupted session with `csa run --session <session-id> ...`.",
                                    signal_name, signal_exit_code
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
                if runtime_fallback_enabled
                    && runtime_fallback_attempts < max_runtime_fallback_attempts
                {
                    if let Some(next_tool) = take_next_runtime_fallback_tool(
                        &mut runtime_fallback_candidates,
                        current_tool,
                        &tried_tools,
                    ) {
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
                }
                cleanup_pre_created_fork_session(
                    &mut pre_created_fork_session_id,
                    request.project_root,
                );
                return Err(e);
            }
        };

        if exec_result.exit_code != 0
            && runtime_fallback_enabled
            && runtime_fallback_attempts < max_runtime_fallback_attempts
            && !is_post_run_commit_policy_block(&exec_result.summary)
        {
            if let Some(next_tool) = take_next_runtime_fallback_tool(
                &mut runtime_fallback_candidates,
                current_tool,
                &tried_tools,
            ) {
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
        }

        if is_post_run_commit_policy_block(&exec_result.summary) {
            break exec_result;
        }

        match evaluate_rate_limit_failover(
            tool_name_str,
            &exec_result,
            attempts,
            max_failover_attempts,
            &mut tried_tools,
            executed_session_id.as_deref(),
            effective_session_arg.as_deref(),
            request.ephemeral,
            request.prompt_text,
            request.project_root,
            request.config,
        )? {
            RateLimitAction::Retry {
                new_tool,
                new_model_spec,
            } => {
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
            _ => break exec_result,
        }
    };

    Ok(RunLoopCompletion::Completed(Box::new(RunLoopOutcome {
        result,
        current_tool,
        executed_session_id,
        fork_resolution,
    })))
}

fn persist_fork_timeout_result_if_missing(
    project_root: &Path,
    is_fork: bool,
    tool: ToolName,
    session_id: Option<&str>,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    timeout_seconds: u64,
) {
    if !is_fork {
        return;
    }
    let Some(session_id) = session_id else {
        return;
    };

    let err = anyhow::anyhow!(
        "wall-clock timeout interrupted forked execution before normal finalization after {timeout_seconds}s"
    );
    crate::pipeline_post_exec::ensure_terminal_result_for_session_on_post_exec_error(
        project_root,
        session_id,
        tool.as_str(),
        execution_start_time,
        &err,
    );
}
