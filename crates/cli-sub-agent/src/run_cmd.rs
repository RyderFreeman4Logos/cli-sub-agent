//! `csa run` command handler.
//!
//! Extracted from main.rs to keep file sizes manageable.

use std::time::Instant;

use anyhow::Result;
use tempfile::TempDir;
use tracing::{info, warn};

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolArg, ToolSelectionStrategy};
use csa_executor::structured_output_instructions_for_fork_call;
use csa_lock::SessionLock;
use csa_lock::slot::{
    SlotAcquireResult, ToolSlot, acquire_slot_blocking, format_slot_diagnostic, slot_usage,
    try_acquire_slot,
};

use crate::cli::ReturnTarget;
use crate::pipeline;
use crate::run_cmd_fork::{
    ForkResolution, cleanup_pre_created_fork_session, pre_create_native_fork_session, resolve_fork,
    try_auto_seed_fork,
};
use crate::run_cmd_post::{handle_fork_call_resume, mark_seed_and_evict, update_fork_genealogy};
use crate::run_cmd_tool_selection::{
    resolve_last_session_selection, resolve_return_target_session_id, resolve_skill_and_prompt,
    resolve_slot_wait_timeout_seconds, resolve_tool_by_strategy, take_next_runtime_fallback_tool,
};
use crate::run_helpers::{is_tool_binary_available, parse_tool_name};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_run(
    tool: Option<ToolArg>,
    skill: Option<String>,
    prompt: Option<String>,
    session_arg: Option<String>,
    last: bool,
    fork_from: Option<String>,
    fork_last: bool,
    description: Option<String>,
    fork_call: bool,
    return_to: Option<String>,
    parent: Option<String>,
    ephemeral: bool,
    cd: Option<String>,
    model_spec: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    force: bool,
    force_override_user_config: bool,
    no_failover: bool,
    wait: bool,
    idle_timeout: Option<u64>,
    no_idle_timeout: bool,
    no_memory: bool,
    memory_query: Option<String>,
    current_depth: u32,
    output_format: OutputFormat,
    stream_mode: csa_process::StreamMode,
) -> Result<i32> {
    // 1. Determine project root
    let project_root = pipeline::determine_project_root(cd.as_deref())?;

    // Emit deprecation warnings for legacy resume flags
    if last {
        warn!("--last is deprecated: use --fork-last instead (fork-first architecture)");
        eprintln!(
            "warning: --last is deprecated and will be removed in a future release. Use --fork-last instead."
        );
    }
    if session_arg.is_some() {
        warn!("--session is deprecated: use --fork-from instead (fork-first architecture)");
        eprintln!(
            "warning: --session is deprecated and will be removed in a future release. Use --fork-from instead."
        );
    }

    let return_target = if fork_call {
        Some(match return_to.as_deref() {
            Some(value) => crate::cli::parse_return_to(value)?,
            None => ReturnTarget::Auto,
        })
    } else {
        None
    };

    // 2. Resolve fork flags or legacy resume flags to session ID
    let mut is_fork = fork_from.is_some() || fork_last;
    let mut session_arg = if fork_last {
        info!("Resolving --fork-last to most recent session");
        let sessions = csa_session::list_sessions(&project_root, None)?;
        let (selected_id, ambiguity_warning) = resolve_last_session_selection(sessions)?;
        if let Some(warning) = ambiguity_warning {
            eprintln!("{warning}");
        }
        Some(selected_id)
    } else if fork_from.is_some() {
        info!(fork_from = ?fork_from, "Forking from specified session");
        fork_from
    } else if last {
        let sessions = csa_session::list_sessions(&project_root, None)?;
        let (selected_id, ambiguity_warning) = resolve_last_session_selection(sessions)?;
        if let Some(warning) = ambiguity_warning {
            eprintln!("{warning}");
        }
        Some(selected_id)
    } else {
        session_arg
    };

    // Fork-call always runs as a forked child and optionally returns to a parent session.
    if fork_call {
        let parent_session_id = resolve_return_target_session_id(
            return_target
                .as_ref()
                .expect("return target should be present for fork-call"),
            &project_root,
            session_arg.as_deref(),
            parent.as_deref(),
        )?;

        if session_arg.is_none() {
            if let Some(ref parent_id) = parent_session_id {
                session_arg = Some(parent_id.clone());
            } else {
                anyhow::bail!(
                    "fork-call requires a source session: provide --fork-from/--fork-last, \
                     or set --return-to/--parent/CSA_SESSION_ID"
                );
            }
        }

        is_fork = true;
    }

    // 3. Load configs and validate recursion depth
    let Some((config, global_config)) = pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };

    // 4-6. Resolve skill, build prompt, apply agent config overrides
    let skill_res = resolve_skill_and_prompt(
        skill.as_deref(),
        prompt,
        tool,
        model,
        thinking,
        &project_root,
    )?;
    let prompt_text = skill_res.prompt_text;
    let resolved_skill = skill_res.resolved_skill;
    let skill_agent = resolved_skill.as_ref().and_then(|sk| sk.agent_config());
    let thinking = skill_res.thinking;
    let model = skill_res.model;

    let strategy = skill_res.tool.unwrap_or(ToolArg::Auto).into_strategy();
    let idle_timeout_seconds = if no_idle_timeout {
        info!("Idle timeout disabled via --no-idle-timeout");
        u64::MAX
    } else {
        pipeline::resolve_idle_timeout_seconds(config.as_ref(), idle_timeout)
    };

    // 7. Resolve initial tool based on strategy
    let strategy_result = resolve_tool_by_strategy(
        &strategy,
        model_spec.as_deref(),
        model.as_deref(),
        config.as_ref(),
        &global_config,
        &project_root,
        force,
        force_override_user_config,
    )?;
    let mut heterogeneous_runtime_fallback_candidates = strategy_result.runtime_fallback_candidates;
    let resolved_model_spec = strategy_result.model_spec;
    let resolved_model = strategy_result.model;
    let resolved_tool = strategy_result.tool;

    // Auto seed fork: if no explicit fork/session requested, try to fork from a warm seed
    let seed_result = try_auto_seed_fork(
        &project_root,
        &resolved_tool,
        config.as_ref(),
        is_fork,
        session_arg,
        ephemeral,
    );
    let mut is_auto_seed_fork = seed_result.is_auto_seed_fork;
    is_fork = seed_result.is_fork;
    session_arg = seed_result.session_arg;

    let mut _fork_call_parent_lock: Option<SessionLock> = None;
    let mut fork_call_parent_session_id: Option<String> = None;
    if fork_call {
        let resolved_parent_id = resolve_return_target_session_id(
            return_target
                .as_ref()
                .expect("return target should be present for fork-call"),
            &project_root,
            session_arg.as_deref(),
            parent.as_deref(),
        )?;
        let Some(parent_id) = resolved_parent_id else {
            anyhow::bail!("unable to resolve parent session for fork-call return");
        };

        let state_root = csa_session::get_session_root(&project_root)?;
        _fork_call_parent_lock = Some(csa_lock::acquire_parent_fork_lock(
            &state_root,
            &parent_id,
            "fork-call parent serialization",
        )?);

        let mut parent_state = csa_session::load_session(&project_root, &parent_id)?;
        parent_state
            .record_fork_call_attempt(Instant::now())
            .map_err(anyhow::Error::msg)?;
        csa_session::save_session(&parent_state)?;
        fork_call_parent_session_id = Some(parent_id.clone());

        // If fork source was not explicitly provided, fork from the return parent.
        if session_arg.is_none() {
            session_arg = Some(parent_id);
            is_fork = true;
        }
    }

    // Fork resolution is deferred until after slot acquisition and pre-execution
    // guards to avoid orphaning transport-level forks when a pre-run check fails.
    let mut fork_resolution: Option<ForkResolution> = None;

    // When forking, don't pass session_arg to execute_with_session (that would resume
    // the *source* session). Instead, create a new session with fork genealogy.
    // For native forks, the provider_session_id is pre-populated before execution so
    // that ACP can resume from the forked provider session on the first turn.
    let mut effective_session_arg = if is_fork { None } else { session_arg.clone() };

    // Hint: suggest reusable sessions when creating a new session (only if not auto-forking)
    if effective_session_arg.is_none() && !is_fork {
        let tool_names = vec![resolved_tool.as_str().to_string()];
        match csa_scheduler::session_reuse::find_reusable_sessions(
            &project_root,
            "run",
            &tool_names,
        ) {
            Ok(candidates) if !candidates.is_empty() => {
                let best = &candidates[0];
                eprintln!(
                    "hint: reusable session available for {}: --fork-from {}",
                    best.tool_name,
                    best.session_id.get(..8).unwrap_or(&best.session_id),
                );
            }
            _ => {}
        }
    }

    // Determine max failover attempts from tier config
    let max_failover_attempts = if no_failover {
        1
    } else {
        config
            .as_ref()
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

    // Resolve tier name for TaskContext (skill override > tier_mapping lookup)
    let resolved_tier_name: Option<String> =
        skill_agent.and_then(|a| a.tier.clone()).or_else(|| {
            config.as_ref().and_then(|cfg| {
                cfg.tier_mapping.get("default").cloned().or_else(|| {
                    if cfg.tiers.contains_key("tier3") {
                        Some("tier3".to_string())
                    } else {
                        cfg.tiers.keys().next().cloned()
                    }
                })
            })
        });
    let context_load_options = skill_agent
        .and_then(|agent| pipeline::context_load_options_with_skips(&agent.skip_context));

    // Resolve slots directory
    let slots_dir = GlobalConfig::slots_dir()?;

    // Failover state
    let mut current_tool = resolved_tool;
    let mut current_model_spec = resolved_model_spec;
    let mut current_model = resolved_model;
    let mut tried_tools: Vec<String> = Vec::new();
    let mut attempts = 0;
    let runtime_fallback_enabled =
        matches!(strategy, ToolSelectionStrategy::HeterogeneousPreferred) && !no_failover;
    let mut runtime_fallback_attempts = 0u8;
    let max_runtime_fallback_attempts = 1u8;
    let mut executed_session_id: Option<String> = None;
    let memory_injection = pipeline::MemoryInjectionOptions {
        disabled: no_memory,
        query_override: memory_query,
    };
    // Track pre-created fork session IDs so we can clean them up on failure.
    let mut pre_created_fork_session_id: Option<String> = None;

    let result = loop {
        attempts += 1;

        let executor = pipeline::build_and_validate_executor(
            &current_tool,
            current_model_spec.as_deref(),
            current_model.as_deref(),
            thinking.as_deref(),
            pipeline::ConfigRefs {
                project: config.as_ref(),
                global: Some(&global_config),
            },
            !force, // enforce tier whitelist unless --force
            force_override_user_config,
        )
        .await?;

        // Acquire global slot
        let tool_name_str = executor.tool_name();
        let max_concurrent = global_config.max_concurrent(tool_name_str);
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
                let all_tools = global_config.all_tool_slots();
                let all_tools_ref: Vec<(&str, u32)> =
                    all_tools.iter().map(|(n, m)| (*n, *m)).collect();
                let all_usage = slot_usage(&slots_dir, &all_tools_ref);
                let diag_msg = format_slot_diagnostic(tool_name_str, &status, &all_usage);

                if !no_failover && attempts < max_failover_attempts {
                    let free_alt = all_usage.iter().find(|s| {
                        s.tool_name != tool_name_str
                            && s.free() > 0
                            && !tried_tools.contains(&s.tool_name)
                            && config
                                .as_ref()
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
                        // Clear fork metadata: forks are tool-specific and cannot
                        // transfer across tools. The next iteration will resolve
                        // a fresh fork for the new tool if is_fork is set.
                        fork_resolution = None;
                        // Only reset session arg for fork flows -- fork-created
                        // sessions are tool-specific and cannot transfer. Non-fork
                        // resumed sessions (--session/--last) must keep their
                        // session context to maintain continuity.
                        if is_fork {
                            effective_session_arg = None;
                        }
                        continue;
                    }
                }

                if wait {
                    info!(
                        tool = %tool_name_str,
                        "All slots occupied, waiting for a free slot"
                    );
                    let timeout = std::time::Duration::from_secs(
                        resolve_slot_wait_timeout_seconds(config.as_ref()),
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
                    return Ok(1);
                }
            }
        }

        // Fork-call slot discipline:
        // 1) release orchestrator/parent hold,
        // 2) reacquire for child execution,
        // so max_concurrent=1 does not deadlock parent->child flows.
        if fork_call {
            let slot_timeout = resolve_slot_wait_timeout_seconds(config.as_ref());
            match crate::run_cmd_fork::fork_call_slot_handoff(
                &mut _slot_guard,
                &slots_dir,
                tool_name_str,
                max_concurrent,
                wait,
                slot_timeout,
                session_arg.as_deref(),
            ) {
                Ok(child_slot) => _slot_guard = Some(child_slot),
                Err(e) => {
                    eprintln!("{}", e);
                    return Ok(1);
                }
            }
        }

        // Resolve fork lazily: only after slot acquisition confirms we will proceed.
        // This prevents orphaning transport-level forks when pre-run checks fail.
        if is_fork && fork_resolution.is_none() {
            if let Some(ref source_id) = session_arg {
                let codex_auto_trust = config.as_ref().is_some_and(ProjectConfig::codex_auto_trust);
                match resolve_fork(
                    source_id,
                    current_tool.as_str(),
                    &project_root,
                    codex_auto_trust,
                )
                .await
                {
                    Ok(res) => fork_resolution = Some(res),
                    Err(e) if is_auto_seed_fork => {
                        // Auto seed forks are best-effort: degrade to cold start.
                        // Clear all fork intent so retries don't re-enter fork resolution.
                        warn!(
                            error = %e,
                            source = %source_id,
                            "Auto seed fork resolution failed, falling back to cold start"
                        );
                        is_auto_seed_fork = false;
                        is_fork = false;
                        session_arg = None;
                        // fall through with fork_resolution = None; handled below
                    }
                    Err(e) => return Err(e),
                }
            } else if !is_auto_seed_fork {
                anyhow::bail!("Fork requested but no source session resolved");
            }
        }

        // For native forks: pre-create a session with the forked provider_session_id
        // in tool state so that ACP can resume from the forked provider session.
        if let Some(ref fork_res) = fork_resolution {
            let (pre_id, new_eff) = pre_create_native_fork_session(
                &project_root,
                fork_res,
                &current_tool,
                description.as_deref(),
                effective_session_arg,
            )?;
            if pre_id.is_some() {
                pre_created_fork_session_id = pre_id;
            }
            effective_session_arg = new_eff;
        }

        let extra_env = global_config.env_vars(tool_name_str).cloned();

        // Prepend soft fork context to prompt if applicable.
        let mut effective_prompt = if let Some(ref fork_res) = fork_resolution {
            if let Some(ref ctx) = fork_res.context_prefix {
                info!(
                    context_len = ctx.len(),
                    "Prepending soft fork context to prompt"
                );
                format!("{ctx}\n\n---\n\n{prompt_text}")
            } else {
                prompt_text.clone()
            }
        } else {
            prompt_text.clone()
        };

        if fork_call && let Some(instructions) = structured_output_instructions_for_fork_call(true)
        {
            effective_prompt.push_str(instructions);
        }

        // Execute
        let exec_result = if ephemeral {
            let temp_dir = TempDir::new()?;
            info!("Ephemeral session in: {:?}", temp_dir.path());
            executor
                .execute_in(
                    &effective_prompt,
                    temp_dir.path(),
                    extra_env.as_ref(),
                    stream_mode,
                    idle_timeout_seconds,
                )
                .await
        } else {
            // Build fork-aware description and parent
            let effective_description = if let Some(ref fork_res) = fork_resolution {
                description.clone().or_else(|| {
                    Some(format!(
                        "fork of {}",
                        fork_res
                            .source_session_id
                            .get(..8)
                            .unwrap_or(&fork_res.source_session_id)
                    ))
                })
            } else {
                description.clone()
            };
            let effective_parent = if let Some(ref fork_res) = fork_resolution {
                Some(fork_res.source_session_id.clone())
            } else {
                parent.clone()
            };

            match pipeline::execute_with_session_and_meta(
                &executor,
                &current_tool,
                &effective_prompt,
                effective_session_arg.clone(),
                effective_description,
                effective_parent,
                &project_root,
                config.as_ref(),
                extra_env.as_ref(),
                Some("run"),
                resolved_tier_name.as_deref(),
                context_load_options.as_ref(),
                stream_mode,
                idle_timeout_seconds,
                Some(&memory_injection),
                Some(&global_config),
            )
            .await
            {
                Ok(session_result) => {
                    executed_session_id = Some(session_result.meta_session_id);
                    Ok(session_result.execution)
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    if error_msg.contains("Session locked by PID")
                        && matches!(output_format, OutputFormat::Json)
                    {
                        cleanup_pre_created_fork_session(
                            &mut pre_created_fork_session_id,
                            &project_root,
                        );
                        let json_error = serde_json::json!({
                            "error": "session_locked",
                            "session_id": effective_session_arg.unwrap_or_else(|| "(new)".to_string()),
                            "tool": current_tool.as_str(),
                            "message": error_msg
                        });
                        println!("{}", serde_json::to_string_pretty(&json_error)?);
                        return Ok(1);
                    }
                    Err(e)
                }
            }
        };

        let exec_result = match exec_result {
            Ok(result) => result,
            Err(e) => {
                if runtime_fallback_enabled
                    && runtime_fallback_attempts < max_runtime_fallback_attempts
                {
                    if let Some(next_tool) = take_next_runtime_fallback_tool(
                        &mut heterogeneous_runtime_fallback_candidates,
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
                        // Clear fork metadata: forks are tool-specific and cannot
                        // transfer across tools. The next iteration will resolve
                        // a fresh fork for the new tool if is_fork is set.
                        fork_resolution = None;
                        if is_fork {
                            effective_session_arg = None;
                        }
                        cleanup_pre_created_fork_session(
                            &mut pre_created_fork_session_id,
                            &project_root,
                        );
                        continue;
                    }
                }
                cleanup_pre_created_fork_session(&mut pre_created_fork_session_id, &project_root);
                return Err(e);
            }
        };

        // Runtime failure fallback for HeterogeneousPreferred:
        // one retry using the next heterogeneous candidate on non-zero exit.
        if exec_result.exit_code != 0
            && runtime_fallback_enabled
            && runtime_fallback_attempts < max_runtime_fallback_attempts
        {
            if let Some(next_tool) = take_next_runtime_fallback_tool(
                &mut heterogeneous_runtime_fallback_candidates,
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
                // Clear fork metadata: forks are tool-specific and cannot
                // transfer across tools. The next iteration will resolve
                // a fresh fork for the new tool if is_fork is set.
                fork_resolution = None;
                if is_fork {
                    effective_session_arg = None;
                }
                cleanup_pre_created_fork_session(&mut pre_created_fork_session_id, &project_root);
                continue;
            }
        }

        // Check for 429 rate limit and attempt failover
        match crate::run_cmd_post::evaluate_rate_limit_failover(
            tool_name_str,
            &exec_result,
            attempts,
            max_failover_attempts,
            &mut tried_tools,
            executed_session_id.as_deref(),
            effective_session_arg.as_deref(),
            ephemeral,
            &prompt_text,
            &project_root,
            config.as_ref(),
        )? {
            crate::run_cmd_post::RateLimitAction::Retry {
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
                cleanup_pre_created_fork_session(&mut pre_created_fork_session_id, &project_root);
                continue;
            }
            _ => break exec_result,
        }
    };

    if fork_call {
        let parent_session_id = fork_call_parent_session_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("fork-call parent session is unresolved"))?;
        handle_fork_call_resume(
            &project_root,
            executed_session_id.as_deref(),
            &parent_session_id,
            &current_tool,
            return_target.is_some(),
            config.as_ref(),
            &global_config,
        )?;
    }

    // Update fork genealogy on the executed session (post-execution).
    if let Some(ref fork_res) = fork_resolution {
        if let Some(ref sid) = executed_session_id {
            update_fork_genealogy(&project_root, sid, fork_res, &current_tool);
        }
    }

    // Mark successful non-fork sessions as seed candidates and run LRU eviction.
    if result.exit_code == 0 && fork_resolution.is_none() && !ephemeral {
        if let Some(ref sid) = executed_session_id {
            mark_seed_and_evict(&project_root, sid, &current_tool, config.as_ref());
        }
    }

    // Print result
    match output_format {
        OutputFormat::Text => {
            print!("{}", result.output);
        }
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&result)?;
            println!("{}", json);
        }
    }

    Ok(result.exit_code)
}

#[cfg(test)]
#[path = "run_cmd_tests.rs"]
mod tests;
