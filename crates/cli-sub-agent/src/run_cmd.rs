//! `csa run` command handler.
//!
//! Extracted from main.rs to keep file sizes manageable.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use tempfile::TempDir;
use tracing::{info, warn};

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolArg, ToolName, ToolSelectionStrategy};
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

const DEFAULT_PR_CODEX_BOT_TIMEOUT_SECS: u64 = 2400;
const RUN_TIMEOUT_EXIT_CODE: i32 = 124;
const POST_RUN_POLICY_BLOCKED_SUMMARY: &str =
    "post-run policy blocked: workspace mutated without commit";
const POST_RUN_POLICY_UNVERIFIABLE_SUMMARY: &str =
    "post-run policy blocked: unable to verify workspace mutation state";
const POST_RUN_POLICY_FORBIDDEN_NO_VERIFY_SUMMARY: &str =
    "post-run policy blocked: forbidden git commit --no-verify detected";
const ALLOW_NO_VERIFY_COMMIT_MARKER: &str = "allow_git_commit_no_verify=1";

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
    timeout: Option<u64>,
    no_idle_timeout: bool,
    no_memory: bool,
    memory_query: Option<String>,
    current_depth: u32,
    output_format: OutputFormat,
    stream_mode: csa_process::StreamMode,
) -> Result<i32> {
    // 1. Determine project root
    let project_root = pipeline::determine_project_root(cd.as_deref())?;
    let effective_repo =
        detect_effective_repo(&project_root).unwrap_or_else(|| "(unknown)".to_string());
    eprintln!(
        "csa run context: effective_repo={} effective_cwd={}",
        effective_repo,
        project_root.display()
    );

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
    let skill_session_tag = skill.as_deref().map(skill_session_description);

    let strategy = skill_res.tool.unwrap_or(ToolArg::Auto).into_strategy();
    let idle_timeout_seconds = if no_idle_timeout {
        info!("Idle timeout disabled via --no-idle-timeout");
        u64::MAX
    } else {
        pipeline::resolve_idle_timeout_seconds(config.as_ref(), idle_timeout)
    };
    let run_timeout_seconds = resolve_run_timeout_seconds(timeout, skill.as_deref());
    let run_started_at = Instant::now();
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

    if session_arg.is_none()
        && !is_fork
        && !fork_call
        && !ephemeral
        && let Some(skill_name) = skill.as_deref()
        && let Some(interrupted_session_id) =
            find_recent_interrupted_skill_session(&project_root, skill_name, &resolved_tool)
    {
        eprintln!(
            "Auto-resuming interrupted skill session {} for '{}'.",
            interrupted_session_id, skill_name
        );
        session_arg = Some(interrupted_session_id);
    }

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

        let remaining_run_timeout =
            resolve_remaining_run_timeout(run_timeout_seconds, run_started_at);
        if remaining_run_timeout.is_some_and(|remaining| remaining.is_zero()) {
            let timeout_resume_session = executed_session_id
                .clone()
                .or_else(|| pre_created_fork_session_id.clone())
                .or_else(|| effective_session_arg.clone());
            return emit_run_timeout(
                output_format,
                run_timeout_seconds.expect("run timeout should be present"),
                current_tool,
                skill.as_deref(),
                timeout_resume_session.as_deref(),
            );
        }

        // Execute
        let exec_result = if ephemeral {
            let temp_dir = TempDir::new()?;
            info!("Ephemeral session in: {:?}", temp_dir.path());
            if let Some(timeout_duration) = remaining_run_timeout {
                match tokio::time::timeout(
                    timeout_duration,
                    executor.execute_in(
                        &effective_prompt,
                        temp_dir.path(),
                        extra_env.as_ref(),
                        stream_mode,
                        idle_timeout_seconds,
                    ),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        let timeout_resume_session = executed_session_id
                            .clone()
                            .or_else(|| pre_created_fork_session_id.clone())
                            .or_else(|| effective_session_arg.clone());
                        return emit_run_timeout(
                            output_format,
                            run_timeout_seconds.expect("run timeout should be present"),
                            current_tool,
                            skill.as_deref(),
                            timeout_resume_session.as_deref(),
                        );
                    }
                }
            } else {
                executor
                    .execute_in(
                        &effective_prompt,
                        temp_dir.path(),
                        extra_env.as_ref(),
                        stream_mode,
                        idle_timeout_seconds,
                    )
                    .await
            }
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
                description.clone().or_else(|| skill_session_tag.clone())
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
                output_format,
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
                remaining_run_timeout,
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
                if let Some(timeout_secs) =
                    wall_timeout_seconds_from_error(&e).or(run_timeout_seconds)
                {
                    let interrupted_session_id = extract_meta_session_id_from_error(&e)
                        .or_else(|| executed_session_id.clone())
                        .or_else(|| pre_created_fork_session_id.clone())
                        .or_else(|| effective_session_arg.clone());
                    return emit_run_timeout(
                        output_format,
                        timeout_secs,
                        current_tool,
                        skill.as_deref(),
                        interrupted_session_id.as_deref(),
                    );
                }
                if let Some(signal_exit_code) = signal_interruption_exit_code(&e) {
                    cleanup_pre_created_fork_session(
                        &mut pre_created_fork_session_id,
                        &project_root,
                    );
                    let interrupted_session_id = extract_meta_session_id_from_error(&e)
                        .or_else(|| executed_session_id.clone())
                        .or_else(|| effective_session_arg.clone());
                    let signal_name = signal_name_from_exit_code(signal_exit_code);

                    match output_format {
                        OutputFormat::Text => {
                            if let Some(ref session_id) = interrupted_session_id {
                                let resume_hint = build_resume_hint_command(
                                    session_id,
                                    current_tool,
                                    skill.as_deref(),
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
                                build_resume_hint_command(
                                    session_id,
                                    current_tool,
                                    skill.as_deref(),
                                )
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

                    return Ok(signal_exit_code);
                }
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
            && !is_post_run_commit_policy_block(&exec_result.summary)
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

        // Commit-policy failures are terminal and must not be bypassed by rate-limit failover.
        if is_post_run_commit_policy_block(&exec_result.summary) {
            break exec_result;
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

fn resolve_run_timeout_seconds(cli_timeout: Option<u64>, skill: Option<&str>) -> Option<u64> {
    if cli_timeout.is_some() {
        return cli_timeout;
    }
    if matches!(skill, Some("pr-codex-bot")) {
        return Some(DEFAULT_PR_CODEX_BOT_TIMEOUT_SECS);
    }
    None
}

fn resolve_remaining_run_timeout(
    run_timeout_seconds: Option<u64>,
    run_started_at: Instant,
) -> Option<Duration> {
    run_timeout_seconds
        .map(|seconds| Duration::from_secs(seconds).saturating_sub(run_started_at.elapsed()))
}

fn emit_run_timeout(
    output_format: OutputFormat,
    timeout_seconds: u64,
    tool: ToolName,
    skill: Option<&str>,
    session_id: Option<&str>,
) -> Result<i32> {
    let message = format!(
        "csa run exceeded wall-clock timeout ({}s); execution terminated",
        timeout_seconds
    );
    match output_format {
        OutputFormat::Text => {
            if let Some(sid) = session_id {
                let resume_hint = build_resume_hint_command(sid, tool, skill);
                eprintln!("{message}. Resume with:\n  {resume_hint}");
            } else {
                eprintln!("{message}.");
            }
        }
        OutputFormat::Json => {
            let resume_hint = session_id.map(|sid| build_resume_hint_command(sid, tool, skill));
            let payload = serde_json::json!({
                "error": "timeout",
                "exit_code": RUN_TIMEOUT_EXIT_CODE,
                "timeout_seconds": timeout_seconds,
                "session_id": session_id,
                "resume_hint": resume_hint,
                "message": message,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(RUN_TIMEOUT_EXIT_CODE)
}

fn detect_effective_repo(project_root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }

    // Strip credentials from HTTPS/SSH URLs (e.g. https://user:token@github.com/repo)
    let sanitized = if let Some(pos) = raw.find("://") {
        let (scheme, rest) = raw.split_at(pos + 3);
        if let Some(at_pos) = rest.find('@') {
            format!("{}{}", scheme, &rest[at_pos + 1..])
        } else {
            raw
        }
    } else {
        raw
    };

    let trimmed = sanitized.trim_end_matches(".git");
    if let Some(rest) = trimmed.strip_prefix("git@github.com:") {
        return Some(rest.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        return Some(rest.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("ssh://git@github.com/") {
        return Some(rest.to_string());
    }
    Some(trimmed.to_string())
}

fn signal_interruption_exit_code(error: &anyhow::Error) -> Option<i32> {
    for cause in error.chain() {
        let message = cause.to_string().to_ascii_lowercase();
        if message.contains("sigterm") {
            return Some(143);
        }
        if message.contains("sigint") {
            return Some(130);
        }
    }
    None
}

fn wall_timeout_seconds_from_error(error: &anyhow::Error) -> Option<u64> {
    const MARKER: &str = "WALL_TIMEOUT timeout_secs=";
    for cause in error.chain() {
        let message = cause.to_string();
        let Some(idx) = message.find(MARKER) else {
            continue;
        };
        let suffix = &message[idx + MARKER.len()..];
        let digits: String = suffix
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();
        if let Ok(value) = digits.parse::<u64>()
            && value > 0
        {
            return Some(value);
        }
    }
    None
}

fn signal_name_from_exit_code(exit_code: i32) -> &'static str {
    match exit_code {
        143 => "SIGTERM",
        130 => "SIGINT",
        _ => "signal",
    }
}

fn extract_meta_session_id_from_error(error: &anyhow::Error) -> Option<String> {
    const MARKER: &str = "meta_session_id=";
    for cause in error.chain() {
        let message = cause.to_string();
        let Some(idx) = message.find(MARKER) else {
            continue;
        };
        let suffix = &message[idx + MARKER.len()..];
        let session_id: String = suffix
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric())
            .collect();
        if !session_id.is_empty() {
            return Some(session_id);
        }
    }
    None
}

fn build_resume_hint_command(session_id: &str, tool: ToolName, skill: Option<&str>) -> String {
    match skill {
        Some(skill_name) => format!(
            "csa run --session {} --tool {} --skill {}",
            session_id,
            tool.as_str(),
            skill_name
        ),
        None => format!(
            "csa run --session {} --tool {} <same prompt>",
            session_id,
            tool.as_str()
        ),
    }
}

fn skill_session_description(skill_name: &str) -> String {
    format!("skill:{skill_name}")
}

fn session_matches_interrupted_skill(
    session: &csa_session::MetaSessionState,
    skill_name: &str,
) -> bool {
    let expected = skill_session_description(skill_name);
    let description_matches = session.description.as_deref() == Some(expected.as_str());
    let terminated_by_signal = matches!(
        session.termination_reason.as_deref(),
        Some("sigterm" | "sigint")
    );
    description_matches && terminated_by_signal
}

fn find_recent_interrupted_skill_session(
    project_root: &Path,
    skill_name: &str,
    tool: &ToolName,
) -> Option<String> {
    let sessions = csa_session::find_sessions(
        project_root,
        None,
        Some("run"),
        None,
        Some(&[tool.as_str()]),
    )
    .ok()?;

    for session in sessions {
        if !session_matches_interrupted_skill(&session, skill_name) {
            continue;
        }

        match csa_session::load_result(project_root, &session.meta_session_id) {
            Ok(Some(result))
                if result.status == "interrupted"
                    || result.exit_code == 130
                    || result.exit_code == 143 =>
            {
                return Some(session.meta_session_id.clone());
            }
            Ok(None) => return Some(session.meta_session_id.clone()),
            Ok(Some(_)) => continue,
            Err(_) => return Some(session.meta_session_id.clone()),
        }
    }

    None
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct GitWorkspaceSnapshot {
    head: Option<String>,
    status: String,
    tracked_worktree_fingerprint: Option<u64>,
    tracked_index_fingerprint: Option<u64>,
    untracked_fingerprint: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PostRunCommitGuard {
    workspace_mutated: bool,
    head_changed: bool,
    changed_paths: Vec<String>,
}

pub(crate) fn capture_git_workspace_snapshot(
    project_root: &Path,
    deep_fingerprint: bool,
) -> Option<GitWorkspaceSnapshot> {
    if !is_git_worktree(project_root) {
        return None;
    }

    let head = run_git_capture(project_root, &["rev-parse", "--verify", "HEAD"])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let status = run_git_capture(
        project_root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--no-renames",
            "-z",
        ],
    )?;
    let tracked_worktree_fingerprint = if deep_fingerprint {
        Some(capture_tracked_worktree_fingerprint(project_root, &status)?)
    } else {
        Some(capture_tracked_worktree_shallow_fingerprint(
            project_root,
            &status,
        ))
    };
    let tracked_index_fingerprint = Some(capture_tracked_index_fingerprint(project_root, &status)?);
    let untracked_fingerprint = Some(capture_untracked_fingerprint(
        project_root,
        deep_fingerprint,
    )?);

    Some(GitWorkspaceSnapshot {
        head,
        status,
        tracked_worktree_fingerprint,
        tracked_index_fingerprint,
        untracked_fingerprint,
    })
}

pub(crate) fn is_git_worktree(project_root: &Path) -> bool {
    run_git_capture(project_root, &["rev-parse", "--is-inside-work-tree"])
        .is_some_and(|value| value.trim() == "true")
}

fn run_git_capture(project_root: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_git_capture_with_paths(
    project_root: &Path,
    fixed_args: &[&str],
    paths: &[String],
) -> Option<String> {
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(project_root).args(fixed_args);
    for path in paths {
        command.arg(path);
    }

    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn hash_text(input: &str) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut hasher);
    hasher.finish()
}

fn capture_untracked_fingerprint(project_root: &Path, deep_content_hash: bool) -> Option<u64> {
    use std::hash::{Hash, Hasher};

    let raw_entries = run_git_capture(
        project_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;

    let paths: Vec<String> = raw_entries
        .split(' ')
        .filter(|entry| !entry.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in &paths {
        path.hash(&mut hasher);
    }

    if deep_content_hash {
        let mut hashable_paths = Vec::new();
        for path in &paths {
            let full_path = project_root.join(path);
            match std::fs::symlink_metadata(&full_path) {
                Ok(metadata) if !metadata.is_dir() => hashable_paths.push(path.clone()),
                Ok(metadata) => {
                    "dir".hash(&mut hasher);
                    metadata.len().hash(&mut hasher);
                    if let Ok(modified) = metadata.modified()
                        && let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH)
                    {
                        since_epoch.as_secs().hash(&mut hasher);
                        since_epoch.subsec_nanos().hash(&mut hasher);
                    }
                }
                Err(_) => {
                    "missing".hash(&mut hasher);
                }
            }
        }

        if !hashable_paths.is_empty() {
            let content_hashes =
                run_git_capture_with_paths(project_root, &["hash-object", "--"], &hashable_paths)?;
            content_hashes.hash(&mut hasher);
        }

        return Some(hasher.finish());
    }

    for path in &paths {
        let full_path = project_root.join(path);
        if let Ok(metadata) = std::fs::metadata(&full_path) {
            metadata.len().hash(&mut hasher);
            if let Ok(modified) = metadata.modified()
                && let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                since_epoch.as_secs().hash(&mut hasher);
                since_epoch.subsec_nanos().hash(&mut hasher);
            }
        }
    }

    Some(hasher.finish())
}

fn capture_tracked_worktree_fingerprint(project_root: &Path, status: &str) -> Option<u64> {
    use std::hash::{Hash, Hasher};

    let paths = tracked_paths_from_status(status, |x, y| x != '?' && y != ' ');
    if paths.is_empty() {
        return Some(0);
    }

    let mut hashable_paths = Vec::new();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in &paths {
        path.hash(&mut hasher);
        let full_path = project_root.join(path);
        match std::fs::symlink_metadata(&full_path) {
            Ok(metadata) if !metadata.is_dir() => hashable_paths.push(path.clone()),
            Ok(metadata) => {
                "dir".hash(&mut hasher);
                metadata.len().hash(&mut hasher);
                if let Ok(modified) = metadata.modified()
                    && let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH)
                {
                    since_epoch.as_secs().hash(&mut hasher);
                    since_epoch.subsec_nanos().hash(&mut hasher);
                }
            }
            Err(_) => {
                "missing".hash(&mut hasher);
            }
        }
    }

    if !hashable_paths.is_empty() {
        let content_hashes =
            run_git_capture_with_paths(project_root, &["hash-object", "--"], &hashable_paths)?;
        content_hashes.hash(&mut hasher);
    }

    Some(hasher.finish())
}

fn capture_tracked_worktree_shallow_fingerprint(project_root: &Path, status: &str) -> u64 {
    let paths = tracked_paths_from_status(status, |x, y| x != '?' && y != ' ');
    hash_paths_and_metadata(project_root, &paths)
}

fn capture_tracked_index_fingerprint(project_root: &Path, status: &str) -> Option<u64> {
    let paths = tracked_paths_from_status(status, |x, _| x != ' ' && x != '?');
    if paths.is_empty() {
        return Some(0);
    }

    run_git_capture_with_paths(project_root, &["ls-files", "--stage", "--"], &paths)
        .map(|output| hash_text(&output))
}

fn tracked_paths_from_status(status: &str, include: impl Fn(char, char) -> bool) -> Vec<String> {
    collect_status_entries(status)
        .into_iter()
        .filter_map(|entry| {
            let (x, y, path) = parse_status_entry(entry)?;
            if !include(x, y) {
                return None;
            }
            Some(path.to_string())
        })
        .collect()
}

fn collect_status_entries(status: &str) -> Vec<&str> {
    if status.contains('\0') {
        status
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .collect()
    } else {
        status.lines().filter(|entry| !entry.is_empty()).collect()
    }
}

fn parse_status_entry(entry: &str) -> Option<(char, char, &str)> {
    let mut chars = entry.chars();
    let x = chars.next()?;
    let y = chars.next()?;
    if chars.next()? != ' ' {
        return None;
    }
    let path = entry.get(3..)?;
    if path.is_empty() {
        return None;
    }
    Some((x, y, path))
}

fn hash_paths_and_metadata(project_root: &Path, paths: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};

    if paths.is_empty() {
        return 0;
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in paths {
        path.hash(&mut hasher);

        let full_path = project_root.join(path);
        if let Ok(metadata) = std::fs::metadata(&full_path) {
            metadata.len().hash(&mut hasher);
            if let Ok(modified) = metadata.modified()
                && let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                since_epoch.as_secs().hash(&mut hasher);
                since_epoch.subsec_nanos().hash(&mut hasher);
            }
        }
    }

    hasher.finish()
}

pub(crate) fn evaluate_post_run_commit_guard(
    before: Option<&GitWorkspaceSnapshot>,
    after: Option<&GitWorkspaceSnapshot>,
) -> Option<PostRunCommitGuard> {
    let after = after?;
    let before = before?;
    if after.status.trim().is_empty() {
        return None;
    }

    let tracked_fingerprint_changed = before.status != after.status
        || before.tracked_worktree_fingerprint != after.tracked_worktree_fingerprint
        || before.tracked_index_fingerprint != after.tracked_index_fingerprint;
    let untracked_changed = before.untracked_fingerprint != after.untracked_fingerprint;
    let workspace_mutated = tracked_fingerprint_changed || untracked_changed;
    if !workspace_mutated {
        return None;
    }

    Some(PostRunCommitGuard {
        workspace_mutated,
        head_changed: before.head != after.head,
        changed_paths: changed_paths_from_status(&after.status, 8),
    })
}

fn changed_paths_from_status(status: &str, limit: usize) -> Vec<String> {
    collect_status_entries(status)
        .into_iter()
        .filter_map(|entry| parse_status_entry(entry).map(|(_, _, path)| path.to_string()))
        .take(limit)
        .collect()
}

fn is_post_run_commit_policy_block(summary: &str) -> bool {
    summary == POST_RUN_POLICY_BLOCKED_SUMMARY
        || summary == POST_RUN_POLICY_UNVERIFIABLE_SUMMARY
        || summary == POST_RUN_POLICY_FORBIDDEN_NO_VERIFY_SUMMARY
}

pub(crate) fn apply_post_run_commit_policy(
    result: &mut csa_process::ExecutionResult,
    output_format: &OutputFormat,
    require_commit_on_mutation: bool,
    commit_guard: Option<&PostRunCommitGuard>,
) {
    let Some(commit_guard) = commit_guard else {
        return;
    };

    let enforce_closed_policy =
        require_commit_on_mutation && commit_guard.workspace_mutated && !commit_guard.head_changed;
    let guard_message = format_post_run_commit_guard_message(commit_guard, enforce_closed_policy);

    if enforce_closed_policy {
        let previous_summary = result.summary.clone();
        if result.exit_code == 0 {
            result.exit_code = 1;
        }
        if !previous_summary.trim().is_empty()
            && previous_summary != POST_RUN_POLICY_BLOCKED_SUMMARY
        {
            append_stderr_block(
                &mut result.stderr_output,
                &format!("Original summary before commit policy: {previous_summary}"),
            );
        }
        result.summary = POST_RUN_POLICY_BLOCKED_SUMMARY.to_string();
    }

    match output_format {
        OutputFormat::Text => eprintln!("{guard_message}"),
        OutputFormat::Json => append_stderr_block(&mut result.stderr_output, &guard_message),
    }
}

pub(crate) fn apply_unverifiable_commit_policy(
    result: &mut csa_process::ExecutionResult,
    output_format: &OutputFormat,
    policy_evaluation_failed: bool,
) {
    if !policy_evaluation_failed {
        return;
    }

    let previous_summary = result.summary.clone();
    if result.exit_code == 0 {
        result.exit_code = 1;
    }
    if !previous_summary.trim().is_empty()
        && previous_summary != POST_RUN_POLICY_UNVERIFIABLE_SUMMARY
    {
        append_stderr_block(
            &mut result.stderr_output,
            &format!("Original summary before commit policy: {previous_summary}"),
        );
    }
    result.summary = POST_RUN_POLICY_UNVERIFIABLE_SUMMARY.to_string();

    let guard_message =
        "ERROR: strict commit policy could not verify workspace mutation state; run is blocked.";
    match output_format {
        OutputFormat::Text => eprintln!("{guard_message}"),
        OutputFormat::Json => append_stderr_block(&mut result.stderr_output, guard_message),
    }
}

pub(crate) fn apply_no_verify_commit_policy(
    result: &mut csa_process::ExecutionResult,
    output_format: &OutputFormat,
    prompt: &str,
    executed_shell_commands: &[String],
    execute_events_observed: bool,
) {
    if prompt_allows_no_verify_commit(prompt) {
        return;
    }

    let mut matched_commands = detect_no_verify_commit_commands(executed_shell_commands);
    if matched_commands.is_empty() && !execute_events_observed {
        matched_commands = detect_no_verify_commit_commands_from_tool_output(
            result,
            !executed_shell_commands.is_empty(),
        );
    }
    if matched_commands.is_empty() {
        return;
    }

    let previous_summary = result.summary.clone();
    if result.exit_code == 0 {
        result.exit_code = 1;
    }
    if !previous_summary.trim().is_empty()
        && previous_summary != POST_RUN_POLICY_FORBIDDEN_NO_VERIFY_SUMMARY
    {
        append_stderr_block(
            &mut result.stderr_output,
            &format!("Original summary before commit policy: {previous_summary}"),
        );
    }
    result.summary = POST_RUN_POLICY_FORBIDDEN_NO_VERIFY_SUMMARY.to_string();

    let mut message = String::from(
        "ERROR: forbidden `git commit --no-verify` (or `git commit -n`) detected in executed shell commands.\n\
If this is intentional, add `ALLOW_GIT_COMMIT_NO_VERIFY=1` to the prompt.\n\
Matched commands:",
    );
    for command in matched_commands {
        message.push_str("\n- ");
        message.push_str(&command);
    }
    match output_format {
        OutputFormat::Text => eprintln!("{message}"),
        OutputFormat::Json => append_stderr_block(&mut result.stderr_output, &message),
    }
}

fn format_post_run_commit_guard_message(
    guard: &PostRunCommitGuard,
    enforce_closed_policy: bool,
) -> String {
    let severity = if enforce_closed_policy {
        "ERROR"
    } else {
        "WARNING"
    };
    let reason = if guard.head_changed {
        "run created commit(s) but still left uncommitted workspace mutations"
    } else {
        "run left uncommitted workspace mutations compared to start"
    };

    let mut lines = vec![format!("{severity}: csa run completed but {reason}.")];
    lines.push(
        "Next step: run `csa run --skill commit \"<scope>\"` and continue with PR/review workflow."
            .to_string(),
    );
    if !guard.changed_paths.is_empty() {
        lines.push(format!("Changed paths: {}", guard.changed_paths.join(", ")));
    }
    lines.join("\n")
}

fn prompt_allows_no_verify_commit(prompt: &str) -> bool {
    prompt.lines().any(|line| {
        let normalized = strip_marker_line_prefix(line).trim().to_ascii_lowercase();
        normalized == ALLOW_NO_VERIFY_COMMIT_MARKER
            || normalized == format!("policy override: {ALLOW_NO_VERIFY_COMMIT_MARKER}")
    })
}

fn strip_marker_line_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    if let Some(stripped) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        return stripped.trim_start();
    }

    let digit_count = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digit_count > 0 {
        let suffix = &trimmed[digit_count..];
        if let Some(stripped) = suffix
            .strip_prefix(". ")
            .or_else(|| suffix.strip_prefix(") "))
        {
            return stripped.trim_start();
        }
    }

    trimmed
}

pub(crate) fn extract_executed_shell_commands_from_events<T: serde::Serialize>(
    events: &[T],
) -> Vec<String> {
    let mut commands = Vec::new();
    for event in events {
        let Ok(value) = serde_json::to_value(event) else {
            continue;
        };
        collect_execute_titles_from_event_value(&value, &mut commands);
    }
    commands
}

pub(crate) fn events_contain_execute_tool_calls<T: serde::Serialize>(events: &[T]) -> bool {
    for event in events {
        let Ok(value) = serde_json::to_value(event) else {
            continue;
        };
        if event_value_contains_execute_kind(&value) {
            return true;
        }
    }
    false
}

fn event_value_contains_execute_kind(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            if map
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|kind| kind.eq_ignore_ascii_case("execute"))
            {
                return true;
            }
            map.values().any(event_value_contains_execute_kind)
        }
        serde_json::Value::Array(values) => values.iter().any(event_value_contains_execute_kind),
        _ => false,
    }
}

fn collect_execute_titles_from_event_value(value: &serde_json::Value, commands: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            let kind = map.get("kind").and_then(serde_json::Value::as_str);
            let title = map.get("title").and_then(serde_json::Value::as_str);
            if let (Some(kind), Some(title)) = (kind, title)
                && kind.eq_ignore_ascii_case("execute")
            {
                let command = title.trim();
                if !command.is_empty() && !commands.iter().any(|existing| existing == command) {
                    commands.push(command.to_string());
                }
            }
            for child in map.values() {
                collect_execute_titles_from_event_value(child, commands);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                collect_execute_titles_from_event_value(child, commands);
            }
        }
        _ => {}
    }
}

fn detect_no_verify_commit_commands(executed_shell_commands: &[String]) -> Vec<String> {
    let mut matches = Vec::new();
    for command in executed_shell_commands {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !command_contains_forbidden_no_verify_commit(trimmed) {
            continue;
        }
        if !matches.iter().any(|existing| existing == trimmed) {
            matches.push(trimmed.to_string());
        }
    }
    matches
}

fn detect_no_verify_commit_commands_from_tool_output(
    result: &csa_process::ExecutionResult,
    trace_only: bool,
) -> Vec<String> {
    let mut matches = Vec::new();
    collect_no_verify_command_like_lines(&result.output, &mut matches, trace_only);
    collect_no_verify_command_like_lines(&result.summary, &mut matches, trace_only);
    collect_no_verify_command_like_lines(&result.stderr_output, &mut matches, trace_only);
    matches
}

fn collect_no_verify_command_like_lines(source: &str, matches: &mut Vec<String>, trace_only: bool) {
    let mut inside_code_fence = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            inside_code_fence = !inside_code_fence;
            continue;
        }
        if inside_code_fence || trimmed.is_empty() {
            continue;
        }
        if trace_only && !has_command_prompt_prefix(trimmed) {
            continue;
        }
        if !looks_like_shell_command_line(trimmed) {
            continue;
        }
        let normalized_command = strip_command_prompt_prefix(trimmed);
        if !command_contains_forbidden_no_verify_commit(normalized_command) {
            continue;
        }
        if !matches
            .iter()
            .any(|existing| existing == normalized_command)
        {
            matches.push(normalized_command.to_string());
        }
    }
}

fn looks_like_shell_command_line(line: &str) -> bool {
    let command_line = strip_command_prompt_prefix(line);
    let Some(first_token) = command_line.split_whitespace().next() else {
        return false;
    };
    if is_env_assignment(first_token) {
        return true;
    }
    is_git_token(first_token)
        || is_shell_token(first_token)
        || first_token.rsplit('/').next() == Some("env")
        || first_token.rsplit('/').next() == Some("sudo")
        || first_token.eq_ignore_ascii_case("sudo")
        || first_token.eq_ignore_ascii_case("env")
        || first_token.eq_ignore_ascii_case("command")
        || first_token.eq_ignore_ascii_case("time")
}

fn has_command_prompt_prefix(line: &str) -> bool {
    line.starts_with("$ ") || line.starts_with("+ ")
}

fn strip_command_prompt_prefix(line: &str) -> &str {
    line.strip_prefix("$ ")
        .or_else(|| line.strip_prefix("+ "))
        .unwrap_or(line)
}

fn command_contains_forbidden_no_verify_commit(command: &str) -> bool {
    split_shell_segments_preserving_quotes(command)
        .into_iter()
        .any(|segment| segment_contains_forbidden_no_verify_commit(&segment))
}

fn split_shell_segments_preserving_quotes(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        if in_single_quote {
            if ch == '\'' {
                current.push(ch);
                in_single_quote = false;
            } else {
                current.push(ch);
            }
            continue;
        }

        if in_double_quote {
            match ch {
                '"' => {
                    current.push(ch);
                    in_double_quote = false;
                }
                '\\' => escaped = true,
                _ => current.push(ch),
            }
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '\'' => {
                current.push(ch);
                in_single_quote = true;
            }
            '"' => {
                current.push(ch);
                in_double_quote = true;
            }
            '\n' | ';' => push_shell_segment(&mut segments, &mut current),
            '&' | '|' => {
                if chars.peek().is_some_and(|next| *next == ch) {
                    let _ = chars.next();
                }
                push_shell_segment(&mut segments, &mut current);
            }
            _ => current.push(ch),
        }
    }

    push_shell_segment(&mut segments, &mut current);
    segments
}

fn push_shell_segment(segments: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        segments.push(trimmed.to_string());
    }
    current.clear();
}

fn segment_contains_forbidden_no_verify_commit(segment: &str) -> bool {
    let tokens = tokenize_shell_tokens(segment);
    if tokens.is_empty() {
        return false;
    }

    if let Some(shell_script_tokens) = extract_shell_c_payload_tokens(&tokens) {
        if shell_script_contains_forbidden_no_verify_commit(shell_script_tokens) {
            return true;
        }
    }

    let Some((_, git_commit_subcommand_idx)) = locate_git_commit_command(&tokens) else {
        return false;
    };
    commit_args_include_no_verify(&tokens[git_commit_subcommand_idx + 1..])
}

fn tokenize_shell_tokens(segment: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = segment.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            continue;
        }

        if in_single_quote {
            if ch == '\'' {
                in_single_quote = false;
            } else {
                current.push(ch);
            }
            continue;
        }

        if in_double_quote {
            if ch == '"' {
                in_double_quote = false;
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            '\'' => in_single_quote = true,
            '"' => in_double_quote = true,
            ch if ch.is_whitespace() => push_shell_token(&mut tokens, &mut current),
            ';' => {
                push_shell_token(&mut tokens, &mut current);
                tokens.push(";".to_string());
            }
            '&' => {
                push_shell_token(&mut tokens, &mut current);
                if chars.peek().is_some_and(|next| *next == '&') {
                    let _ = chars.next();
                    tokens.push("&&".to_string());
                } else {
                    tokens.push("&".to_string());
                }
            }
            '|' => {
                push_shell_token(&mut tokens, &mut current);
                if chars.peek().is_some_and(|next| *next == '|') {
                    let _ = chars.next();
                    tokens.push("||".to_string());
                } else {
                    tokens.push("|".to_string());
                }
            }
            _ => current.push(ch),
        }
    }

    if escaped {
        current.push('\\');
    }
    push_shell_token(&mut tokens, &mut current);
    tokens
}

fn push_shell_token(tokens: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        tokens.push(trimmed.to_string());
    }
    current.clear();
}

fn extract_shell_c_payload_tokens(tokens: &[String]) -> Option<&[String]> {
    let idx = skip_command_prefix_tokens(tokens, 0);
    if idx + 2 >= tokens.len() {
        return None;
    }
    if !is_shell_token(tokens[idx].as_str()) {
        return None;
    }
    let shell_flag = tokens[idx + 1].as_str();
    if !shell_flag.starts_with('-') || !shell_flag.contains('c') {
        return None;
    }
    Some(&tokens[idx + 2..])
}

fn locate_git_commit_command(tokens: &[String]) -> Option<(usize, usize)> {
    let idx = skip_command_prefix_tokens(tokens, 0);
    if idx >= tokens.len() {
        return None;
    }
    if !is_git_token(tokens[idx].as_str()) {
        return None;
    }
    let subcommand_idx = find_git_commit_subcommand(tokens, idx + 1)?;
    Some((idx, subcommand_idx))
}

fn shell_script_contains_forbidden_no_verify_commit(tokens: &[String]) -> bool {
    let script_tokens = expand_shell_script_tokens(tokens);
    for git_idx in 0..script_tokens.len() {
        if !is_git_token(script_tokens[git_idx].as_str())
            || !is_shell_command_boundary(&script_tokens, git_idx)
        {
            continue;
        }
        let Some(commit_idx) = find_git_commit_subcommand(&script_tokens, git_idx + 1) else {
            continue;
        };
        if commit_args_include_no_verify(&script_tokens[commit_idx + 1..]) {
            return true;
        }
    }
    false
}

fn expand_shell_script_tokens(tokens: &[String]) -> Vec<String> {
    let mut expanded = Vec::new();
    for token in tokens {
        let nested_tokens = tokenize_shell_tokens(token);
        if nested_tokens.is_empty() {
            continue;
        }
        expanded.extend(nested_tokens);
    }
    expanded
}

fn is_shell_command_boundary(tokens: &[String], idx: usize) -> bool {
    idx == 0 || is_command_separator_token(tokens[idx - 1].as_str())
}

fn is_command_separator_token(token: &str) -> bool {
    matches!(token, ";" | "&&" | "||" | "|" | "&")
        || token.ends_with(';')
        || token.ends_with("&&")
        || token.ends_with("||")
        || token.ends_with('|')
        || token.ends_with('&')
}

fn skip_command_prefix_tokens(tokens: &[String], mut idx: usize) -> usize {
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if is_env_assignment(token) {
            idx += 1;
            continue;
        }
        if token.eq_ignore_ascii_case("sudo") || token.rsplit('/').next() == Some("sudo") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, sudo_option_consumes_value);
            continue;
        }
        if token.eq_ignore_ascii_case("env") || token.ends_with("/env") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, env_option_consumes_value);
            while idx < tokens.len() && is_env_assignment(tokens[idx].as_str()) {
                idx += 1;
            }
            continue;
        }
        if token.eq_ignore_ascii_case("command") || token == "--" {
            idx += 1;
            continue;
        }
        if token.eq_ignore_ascii_case("time") {
            idx += 1;
            idx = skip_prefixed_command_options(tokens, idx, |_token| false);
            continue;
        }
        break;
    }
    idx
}

fn find_git_commit_subcommand(tokens: &[String], mut idx: usize) -> Option<usize> {
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if token.eq_ignore_ascii_case("commit") {
            return Some(idx);
        }
        if token == "--" {
            if idx + 1 < tokens.len() && tokens[idx + 1].eq_ignore_ascii_case("commit") {
                return Some(idx + 1);
            }
            return None;
        }
        if !token.starts_with('-') {
            return None;
        }
        if git_global_option_consumes_value(token) && idx + 1 < tokens.len() {
            idx += 2;
            continue;
        }
        idx += 1;
    }
    None
}

fn skip_prefixed_command_options<F>(tokens: &[String], mut idx: usize, consumes_value: F) -> usize
where
    F: Fn(&str) -> bool,
{
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if token == "--" {
            idx += 1;
            break;
        }
        if !token.starts_with('-') {
            break;
        }
        let takes_value = consumes_value(token) && !token.contains('=');
        idx += 1;
        if takes_value && idx < tokens.len() {
            idx += 1;
        }
    }
    idx
}

fn git_global_option_consumes_value(token: &str) -> bool {
    matches!(
        token,
        "-c" | "-C" | "--exec-path" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env"
    )
}

fn env_option_consumes_value(token: &str) -> bool {
    matches!(
        token,
        "-u" | "--unset" | "-C" | "--chdir" | "-S" | "--split-string"
    )
}

fn sudo_option_consumes_value(token: &str) -> bool {
    matches!(
        token,
        "-u" | "--user"
            | "-g"
            | "--group"
            | "-h"
            | "--host"
            | "-p"
            | "--prompt"
            | "-r"
            | "--role"
            | "-t"
            | "--type"
            | "-C"
            | "--chdir"
    )
}

fn is_env_assignment(token: &str) -> bool {
    token
        .find('=')
        .is_some_and(|eq_pos| eq_pos > 0 && !token.starts_with('-'))
}

fn is_shell_token(token: &str) -> bool {
    matches!(
        token.rsplit('/').next(),
        Some("bash" | "sh" | "zsh" | "fish")
    )
}

fn is_git_token(token: &str) -> bool {
    token.eq_ignore_ascii_case("git") || token.ends_with("/git")
}

fn commit_args_include_no_verify(args: &[String]) -> bool {
    let mut idx = 0usize;
    while idx < args.len() {
        let token = args[idx].as_str();
        if token == "--" || is_command_separator_token(token) {
            break;
        }
        if token.eq_ignore_ascii_case("--no-verify") {
            return true;
        }

        if token.starts_with("--") {
            idx += 1;
            if long_option_consumes_value(token) && !token.contains('=') {
                idx = consume_option_value(args, idx, long_option_is_message_like(token));
            }
            continue;
        }

        if token.starts_with('-') && token.len() > 1 {
            let mut chars = token[1..].chars().peekable();
            let mut consumes_value = false;
            let mut message_like = false;
            while let Some(flag) = chars.next() {
                if flag == 'n' {
                    return true;
                }
                if short_option_consumes_value(flag) {
                    consumes_value = chars.peek().is_none();
                    message_like = short_option_is_message_like(flag);
                    break;
                }
            }
            idx += 1;
            if consumes_value {
                idx = consume_option_value(args, idx, message_like);
            }
            continue;
        }

        idx += 1;
    }
    false
}

fn consume_option_value(args: &[String], mut idx: usize, message_like: bool) -> usize {
    if !message_like {
        if idx < args.len() {
            idx += 1;
        }
        return idx;
    }

    if idx < args.len() {
        idx += 1;
    }

    while idx < args.len() {
        let token = args[idx].as_str();
        if is_command_separator_token(token) || token.starts_with('-') {
            break;
        }
        idx += 1;
    }
    idx
}

fn short_option_consumes_value(flag: char) -> bool {
    matches!(flag, 'm' | 'F' | 'c' | 'C' | 't')
}

fn short_option_is_message_like(flag: char) -> bool {
    matches!(flag, 'm')
}

fn long_option_consumes_value(token: &str) -> bool {
    matches!(
        token,
        "--message"
            | "--file"
            | "--template"
            | "--reuse-message"
            | "--reedit-message"
            | "--fixup"
            | "--squash"
            | "--author"
            | "--date"
            | "--trailer"
            | "--pathspec-from-file"
            | "--cleanup"
    )
}

fn long_option_is_message_like(token: &str) -> bool {
    token == "--message"
}

fn append_stderr_block(stderr_output: &mut String, block: &str) {
    if block.trim().is_empty() {
        return;
    }
    if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
        stderr_output.push('\n');
    }
    stderr_output.push_str(block);
    if !stderr_output.ends_with('\n') {
        stderr_output.push('\n');
    }
}

#[cfg(test)]
#[path = "run_cmd_tests.rs"]
mod tests;
