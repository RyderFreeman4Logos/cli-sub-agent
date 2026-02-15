//! `csa run` command handler.
//!
//! Extracted from main.rs to keep file sizes manageable.

use anyhow::Result;
use tempfile::TempDir;
use tracing::{info, warn};

use csa_config::GlobalConfig;
use csa_core::types::{OutputFormat, ToolArg, ToolName, ToolSelectionStrategy};
use csa_lock::slot::{
    SlotAcquireResult, ToolSlot, format_slot_diagnostic, slot_usage, try_acquire_slot,
};
use csa_session::{MetaSessionState, SessionPhase, load_session, resolve_session_prefix};

use crate::pipeline;
use crate::run_helpers::{
    infer_task_edit_requirement, is_tool_binary_available, parse_tool_name, read_prompt,
    resolve_tool, resolve_tool_and_model,
};
use crate::skill_resolver;

fn resolve_last_session_selection(
    sessions: Vec<MetaSessionState>,
) -> Result<(String, Option<String>)> {
    if sessions.is_empty() {
        anyhow::bail!("No sessions found. Run a task first to create one.");
    }

    let mut sorted_sessions = sessions;
    sorted_sessions.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
    let selected_id = sorted_sessions[0].meta_session_id.clone();

    let active_sessions: Vec<&MetaSessionState> = sorted_sessions
        .iter()
        .filter(|session| session.phase == SessionPhase::Active)
        .collect();

    if active_sessions.len() <= 1 {
        return Ok((selected_id, None));
    }

    let mut warning_lines = vec![
        format!(
            "warning: `--last` is ambiguous in this project: found {} active sessions.",
            active_sessions.len()
        ),
        format!("Resuming most recently accessed session: {}", selected_id),
        "Active sessions (session_id | last_accessed):".to_string(),
    ];

    for session in active_sessions {
        warning_lines.push(format!(
            "  {} | {}",
            session.meta_session_id,
            session.last_accessed.to_rfc3339()
        ));
    }

    warning_lines.push("Use `--session <session-id>` to choose explicitly.".to_string());

    Ok((selected_id, Some(warning_lines.join("\n"))))
}

fn resolve_heterogeneous_candidates(
    parent_tool: &ToolName,
    enabled_tools: &[ToolName],
) -> Vec<ToolName> {
    let parent_family = parent_tool.model_family();
    enabled_tools
        .iter()
        .copied()
        .filter(|tool| tool.model_family() != parent_family)
        .collect()
}

fn take_next_runtime_fallback_tool(
    candidates: &mut Vec<ToolName>,
    current_tool: ToolName,
    tried_tools: &[String],
) -> Option<ToolName> {
    while let Some(candidate) = candidates.first().copied() {
        candidates.remove(0);
        if candidate == current_tool {
            continue;
        }
        if tried_tools.iter().any(|tried| tried == candidate.as_str()) {
            continue;
        }
        return Some(candidate);
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_run(
    tool: Option<ToolArg>,
    skill: Option<String>,
    prompt: Option<String>,
    session_arg: Option<String>,
    last: bool,
    description: Option<String>,
    parent: Option<String>,
    ephemeral: bool,
    cd: Option<String>,
    model_spec: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    no_failover: bool,
    wait: bool,
    idle_timeout: Option<u64>,
    current_depth: u32,
    output_format: OutputFormat,
    stream_mode: csa_process::StreamMode,
) -> Result<i32> {
    // 1. Determine project root
    let project_root = pipeline::determine_project_root(cd.as_deref())?;

    // 2. Resolve --last flag to session ID
    let session_arg = if last {
        let sessions = csa_session::list_sessions(&project_root, None)?;
        let (selected_id, ambiguity_warning) = resolve_last_session_selection(sessions)?;
        if let Some(warning) = ambiguity_warning {
            eprintln!("{warning}");
        }
        Some(selected_id)
    } else {
        session_arg
    };

    // 3. Load configs and validate recursion depth
    let Some((config, global_config)) = pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };

    // 4. Resolve --skill if provided
    let resolved_skill = if let Some(ref skill_name) = skill {
        Some(skill_resolver::resolve_skill(skill_name, &project_root)?)
    } else {
        None
    };

    // 5. Read prompt (skill prompt = SKILL.md + extra_context files + optional user prompt)
    let prompt_text = if let Some(ref sk) = resolved_skill {
        let mut parts = vec![sk.skill_md.clone()];

        // Load extra_context files relative to the skill directory.
        if let Some(agent) = sk.agent_config() {
            for extra in &agent.extra_context {
                let extra_path = sk.dir.join(extra);
                match std::fs::read_to_string(&extra_path) {
                    Ok(content) => {
                        parts.push(format!(
                            "<context-file path=\"{}\">\n{}\n</context-file>",
                            extra, content
                        ));
                    }
                    Err(e) => {
                        warn!(path = %extra, error = %e, "Failed to load skill extra_context file");
                    }
                }
            }
        }

        if let Some(user_prompt) = prompt {
            parts.push(format!("---\n\n{}", user_prompt));
        }

        parts.join("\n\n")
    } else {
        read_prompt(prompt)?
    };

    // 6. Apply skill agent config overrides for tool/model when CLI didn't specify.
    let skill_agent = resolved_skill.as_ref().and_then(|sk| sk.agent_config());
    let tool = if tool.is_none() {
        skill_agent
            .and_then(|a| a.tools.first())
            .and_then(|t| parse_tool_name(&t.tool).ok())
            .map(ToolArg::Specific)
            .or(tool)
    } else {
        tool
    };
    let model = if model.is_none() {
        skill_agent
            .and_then(|a| a.tools.first())
            .and_then(|t| t.model.clone())
            .or(model)
    } else {
        model
    };
    let thinking = if thinking.is_none() {
        skill_agent
            .and_then(|a| a.tools.first())
            .and_then(|t| t.thinking_budget.clone())
            .or(thinking)
    } else {
        thinking
    };

    let strategy = tool.unwrap_or(ToolArg::Auto).into_strategy();
    let idle_timeout_seconds =
        pipeline::resolve_idle_timeout_seconds(config.as_ref(), idle_timeout);

    // 7. Resolve initial tool based on strategy
    let mut heterogeneous_runtime_fallback_candidates: Vec<ToolName> = Vec::new();
    let (initial_tool, resolved_model_spec, resolved_model) = match &strategy {
        ToolSelectionStrategy::Explicit(t) => resolve_tool_and_model(
            Some(*t),
            model_spec.as_deref(),
            model.as_deref(),
            config.as_ref(),
            &project_root,
        )?,
        ToolSelectionStrategy::AnyAvailable => resolve_tool_and_model(
            None,
            model_spec.as_deref(),
            model.as_deref(),
            config.as_ref(),
            &project_root,
        )?,
        ToolSelectionStrategy::HeterogeneousPreferred => {
            let detected_parent_tool = crate::run_helpers::detect_parent_tool();
            let parent_tool_name = resolve_tool(detected_parent_tool, &global_config);

            if let Some(parent_str) = parent_tool_name.as_deref() {
                let parent_tool = parse_tool_name(parent_str)?;
                let enabled_tools = if let Some(ref cfg) = config {
                    let tools: Vec<_> = csa_config::global::all_known_tools()
                        .iter()
                        .filter(|t| {
                            cfg.is_tool_auto_selectable(t.as_str())
                                && is_tool_binary_available(t.as_str())
                        })
                        .copied()
                        .collect();
                    csa_config::global::sort_tools_by_effective_priority(
                        &tools,
                        config.as_ref(),
                        &global_config,
                    )
                } else {
                    Vec::new()
                };

                let heterogeneous_candidates =
                    resolve_heterogeneous_candidates(&parent_tool, &enabled_tools);
                match heterogeneous_candidates.first().copied() {
                    Some(tool) => {
                        heterogeneous_runtime_fallback_candidates =
                            heterogeneous_candidates.into_iter().skip(1).collect();
                        resolve_tool_and_model(
                            Some(tool),
                            model_spec.as_deref(),
                            model.as_deref(),
                            config.as_ref(),
                            &project_root,
                        )?
                    }
                    None => {
                        warn!(
                            "No heterogeneous tool available (parent: {}, family: {}). Falling back to any available tool.",
                            parent_tool.as_str(),
                            parent_tool.model_family()
                        );
                        resolve_tool_and_model(
                            None,
                            model_spec.as_deref(),
                            model.as_deref(),
                            config.as_ref(),
                            &project_root,
                        )?
                    }
                }
            } else {
                warn!(
                    "HeterogeneousPreferred requested but no parent tool context/defaults.tool found. Falling back to AnyAvailable."
                );
                resolve_tool_and_model(
                    None,
                    model_spec.as_deref(),
                    model.as_deref(),
                    config.as_ref(),
                    &project_root,
                )?
            }
        }
        ToolSelectionStrategy::HeterogeneousStrict => {
            let detected_parent_tool = crate::run_helpers::detect_parent_tool();
            let parent_tool_name = resolve_tool(detected_parent_tool, &global_config);

            if let Some(parent_str) = parent_tool_name.as_deref() {
                let parent_tool = parse_tool_name(parent_str)?;
                let enabled_tools = if let Some(ref cfg) = config {
                    let tools: Vec<_> = csa_config::global::all_known_tools()
                        .iter()
                        .filter(|t| {
                            cfg.is_tool_auto_selectable(t.as_str())
                                && is_tool_binary_available(t.as_str())
                        })
                        .copied()
                        .collect();
                    csa_config::global::sort_tools_by_effective_priority(
                        &tools,
                        config.as_ref(),
                        &global_config,
                    )
                } else {
                    Vec::new()
                };

                match csa_config::global::select_heterogeneous_tool(&parent_tool, &enabled_tools) {
                    Some(tool) => resolve_tool_and_model(
                        Some(tool),
                        model_spec.as_deref(),
                        model.as_deref(),
                        config.as_ref(),
                        &project_root,
                    )?,
                    None => {
                        anyhow::bail!(
                            "No heterogeneous tool available (parent: {}, family: {}).\n\n\
                             If this is a low-risk task (exploration, documentation, code reading),\n\
                             consider using `--tool any-available` instead.",
                            parent_tool.as_str(),
                            parent_tool.model_family()
                        );
                    }
                }
            } else {
                warn!(
                    "HeterogeneousStrict requested but no parent tool context/defaults.tool found. Falling back to AnyAvailable."
                );
                resolve_tool_and_model(
                    None,
                    model_spec.as_deref(),
                    model.as_deref(),
                    config.as_ref(),
                    &project_root,
                )?
            }
        }
    };

    let resolved_tool = initial_tool;

    // Hint: suggest reusable sessions when creating a new session
    if session_arg.is_none() {
        let tool_names = vec![resolved_tool.as_str().to_string()];
        match csa_scheduler::session_reuse::find_reusable_sessions(
            &project_root,
            "run",
            &tool_names,
        ) {
            Ok(candidates) if !candidates.is_empty() => {
                let best = &candidates[0];
                eprintln!(
                    "hint: reusable session available for {}: --session {}",
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

    let result = loop {
        attempts += 1;

        let executor = pipeline::build_and_validate_executor(
            &current_tool,
            current_model_spec.as_deref(),
            current_model.as_deref(),
            thinking.as_deref(),
            config.as_ref(),
        )
        .await?;

        // Acquire global slot
        let tool_name_str = executor.tool_name();
        let max_concurrent = global_config.max_concurrent(tool_name_str);
        let _slot_guard: Option<ToolSlot>;

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
                        continue;
                    }
                }

                if wait {
                    info!(
                        tool = %tool_name_str,
                        "All slots occupied, waiting for a free slot"
                    );
                    let timeout = std::time::Duration::from_secs(300);
                    let slot = csa_lock::slot::acquire_slot_blocking(
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

        let extra_env = global_config.env_vars(tool_name_str).cloned();

        // Execute
        let exec_result = if ephemeral {
            let temp_dir = TempDir::new()?;
            info!("Ephemeral session in: {:?}", temp_dir.path());
            executor
                .execute_in(
                    &prompt_text,
                    temp_dir.path(),
                    extra_env.as_ref(),
                    stream_mode,
                    idle_timeout_seconds,
                )
                .await
        } else {
            match pipeline::execute_with_session(
                &executor,
                &current_tool,
                &prompt_text,
                session_arg.clone(),
                description.clone(),
                parent.clone(),
                &project_root,
                config.as_ref(),
                extra_env.as_ref(),
                Some("run"),
                resolved_tier_name.as_deref(),
                context_load_options.as_ref(),
                stream_mode,
                idle_timeout_seconds,
                Some(&global_config),
            )
            .await
            {
                Ok(result) => Ok(result),
                Err(e) => {
                    let error_msg = e.to_string();
                    if error_msg.contains("Session locked by PID")
                        && matches!(output_format, OutputFormat::Json)
                    {
                        let json_error = serde_json::json!({
                            "error": "session_locked",
                            "session_id": session_arg.unwrap_or_else(|| "(new)".to_string()),
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
                        continue;
                    }
                }
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
                continue;
            }
        }

        // Check for 429 rate limit and attempt failover
        if let Some(rate_limit) = csa_scheduler::detect_rate_limit(
            tool_name_str,
            &exec_result.stderr_output,
            &exec_result.output,
            exec_result.exit_code,
        ) {
            info!(
                tool = %tool_name_str,
                pattern = %rate_limit.matched_pattern,
                attempt = attempts,
                max = max_failover_attempts,
                "Rate limit detected, attempting failover"
            );

            if attempts >= max_failover_attempts {
                warn!(
                    "Max failover attempts ({}) reached, returning error",
                    max_failover_attempts
                );
                break exec_result;
            }

            tried_tools.push(tool_name_str.to_string());

            let session_state = if !ephemeral {
                session_arg.as_ref().and_then(|sid| {
                    let sessions_dir = csa_session::get_session_root(&project_root)
                        .ok()?
                        .join("sessions");
                    let resolved_id = resolve_session_prefix(&sessions_dir, sid).ok()?;
                    load_session(&project_root, &resolved_id).ok()
                })
            } else {
                None
            };

            let task_needs_edit = infer_task_edit_requirement(&prompt_text).or_else(|| {
                config
                    .as_ref()
                    .map(|cfg| cfg.can_tool_edit_existing(tool_name_str))
            });

            if let Some(ref cfg) = config {
                let action = csa_scheduler::decide_failover(
                    tool_name_str,
                    "default",
                    task_needs_edit,
                    session_state.as_ref(),
                    &tried_tools,
                    cfg,
                    &rate_limit.matched_pattern,
                );

                match action {
                    csa_scheduler::FailoverAction::RetryInSession {
                        new_tool,
                        new_model_spec,
                        session_id: _,
                    }
                    | csa_scheduler::FailoverAction::RetrySiblingSession {
                        new_tool,
                        new_model_spec,
                    } => {
                        info!(
                            from = %tool_name_str,
                            to = %new_tool,
                            "Failing over to alternative tool"
                        );
                        current_tool = parse_tool_name(&new_tool)?;
                        current_model_spec = Some(new_model_spec);
                        current_model = None;
                        continue;
                    }
                    csa_scheduler::FailoverAction::ReportError { reason, .. } => {
                        warn!(reason = %reason, "Failover not possible, returning original result");
                        break exec_result;
                    }
                }
            } else {
                break exec_result;
            }
        } else {
            break exec_result;
        }
    };

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
