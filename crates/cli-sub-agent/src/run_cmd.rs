//! `csa run` command handler.
//!
//! Extracted from main.rs to keep file sizes manageable.

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use tempfile::TempDir;
use tracing::{debug, info, warn};

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolArg, ToolName, ToolSelectionStrategy};
use csa_executor::structured_output_instructions_for_fork_call;
use csa_executor::transport::{ForkMethod, ForkRequest, TransportFactory};
use csa_lock::SessionLock;
use csa_lock::slot::{
    SlotAcquireResult, ToolSlot, acquire_slot_blocking, format_slot_diagnostic, slot_usage,
    try_acquire_slot,
};
use csa_session::{
    MetaSessionState, PhaseEvent, RETURN_PACKET_SECTION_ID, ReturnPacketRef, SessionPhase,
    ToolState, load_output_index, load_session, parse_return_packet, read_section,
    resolve_session_prefix, validate_return_packet_path,
};

use crate::cli::ReturnTarget;
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

/// Remove a pre-created fork session when execution fails or tool failover
/// occurs. Takes the session ID by `&mut Option` so it is consumed (set to
/// `None`) after cleanup, preventing double-delete on subsequent error paths.
fn cleanup_pre_created_fork_session(session_id: &mut Option<String>, project_root: &Path) {
    if let Some(sid) = session_id.take() {
        match csa_session::delete_session(project_root, &sid) {
            Ok(()) => {
                info!(session = %sid, "Cleaned up pre-created fork session after failure");
            }
            Err(e) => {
                warn!(session = %sid, error = %e, "Failed to clean up pre-created fork session");
            }
        }
    }
}

fn resolve_slot_wait_timeout_seconds(config: Option<&ProjectConfig>) -> u64 {
    config
        .map(|cfg| cfg.resources.slot_wait_timeout_seconds)
        .unwrap_or(csa_config::ResourcesConfig::default().slot_wait_timeout_seconds)
}

fn resolve_session_reference(project_root: &Path, session_ref: &str) -> Result<String> {
    let sessions_dir = csa_session::get_session_root(project_root)?.join("sessions");
    resolve_session_prefix(&sessions_dir, session_ref)
}

fn resolve_return_target_session_id(
    return_target: &ReturnTarget,
    project_root: &Path,
    fork_source_ref: Option<&str>,
    parent_flag: Option<&str>,
) -> Result<Option<String>> {
    match return_target {
        ReturnTarget::Last => {
            let sessions = csa_session::list_sessions(project_root, None)?;
            let (selected_id, _) = resolve_last_session_selection(sessions)?;
            Ok(Some(selected_id))
        }
        ReturnTarget::SessionId(session_ref) => {
            let resolved = resolve_session_reference(project_root, session_ref)?;
            Ok(Some(resolved))
        }
        ReturnTarget::Auto => {
            let env_parent = std::env::var("CSA_SESSION_ID").ok();
            let candidate = fork_source_ref
                .map(ToOwned::to_owned)
                .or_else(|| parent_flag.map(ToOwned::to_owned))
                .or(env_parent);

            if let Some(session_ref) = candidate {
                let resolved = resolve_session_reference(project_root, &session_ref)?;
                Ok(Some(resolved))
            } else {
                Ok(None)
            }
        }
    }
}

fn load_child_return_packet(
    project_root: &Path,
    child_session_id: &str,
) -> Result<(csa_session::ReturnPacket, ReturnPacketRef)> {
    let child_session_dir = csa_session::get_session_dir(project_root, child_session_id)?;
    let section_content = read_section(&child_session_dir, RETURN_PACKET_SECTION_ID)?
        .ok_or_else(|| anyhow::anyhow!("child session missing return-packet section"))?;

    let packet = parse_return_packet(&section_content)?;
    for changed in &packet.changed_files {
        if !validate_return_packet_path(&changed.path, project_root) {
            anyhow::bail!(
                "return packet changed file path escapes project root: {}",
                changed.path
            );
        }
    }

    let index = load_output_index(&child_session_dir)?
        .ok_or_else(|| anyhow::anyhow!("missing output/index.toml for child session"))?;
    let section = index
        .sections
        .iter()
        .find(|s| s.id == RETURN_PACKET_SECTION_ID)
        .ok_or_else(|| anyhow::anyhow!("return-packet section not indexed"))?;
    let file_path = section
        .file_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("return-packet section has no file_path"))?;

    let output_dir = child_session_dir
        .join("output")
        .canonicalize()
        .context("failed to canonicalize child output directory")?;
    let section_path = child_session_dir
        .join("output")
        .join(file_path)
        .canonicalize()
        .context("failed to canonicalize return-packet file path")?;
    if !section_path.starts_with(&output_dir) {
        anyhow::bail!("return-packet file resolved outside child output directory");
    }

    Ok((
        packet,
        ReturnPacketRef {
            child_session_id: child_session_id.to_string(),
            section_path: section_path.to_string_lossy().to_string(),
        },
    ))
}

/// Result of resolving a fork request before execution.
struct ForkResolution {
    /// The forked provider session ID (Native fork only).
    provider_session_id: Option<String>,
    /// Context summary to prepend to prompt (Soft fork only).
    context_prefix: Option<String>,
    /// The CSA session ID that was forked from.
    source_session_id: String,
    /// The provider session ID of the source (used to set fork_provider_session_id in genealogy).
    source_provider_session_id: Option<String>,
}

/// Resolve a fork from a source session: run the transport-level fork and return
/// the information needed to create a new CSA session with fork genealogy.
///
/// For soft forks (cross-tool or non-claude-code targets), tool-lock is NOT enforced
/// because soft forks only copy context from the parent session and do not require
/// tool ownership. Native forks (same tool) still enforce tool-lock via
/// `resolve_resume_session`.
async fn resolve_fork(
    source_session_id: &str,
    tool_name: &str,
    project_root: &Path,
    codex_auto_trust: bool,
) -> Result<ForkResolution> {
    // Determine if source session uses a different tool than the target.
    // Cross-tool forks must always use soft fork (context summary injection)
    // because native fork requires the same tool's provider session.
    // When metadata is missing (older/migrated sessions), default to cross-tool
    // (soft fork) as the safe fallback — native fork would fail without metadata.
    let source_tool = csa_session::load_metadata(project_root, source_session_id)
        .ok()
        .flatten()
        .map(|m| m.tool);
    let is_cross_tool = source_tool.as_deref() != Some(tool_name);

    let fork_method = if is_cross_tool {
        ForkMethod::Soft
    } else {
        TransportFactory::fork_method_for_tool(tool_name)
    };

    let resolution = match fork_method {
        ForkMethod::Native => {
            // Native fork requires the same tool's provider session — enforce tool-lock.
            csa_session::resolve_resume_session(project_root, source_session_id, tool_name)?
        }
        ForkMethod::Soft => {
            // Soft fork only reads context files — skip tool-lock enforcement.
            csa_session::resolve_fork_source(project_root, source_session_id)?
        }
    };
    let source_csa_id = resolution.meta_session_id.clone();
    let source_provider_id = resolution.provider_session_id.clone();

    let session_dir = csa_session::get_session_dir(project_root, &source_csa_id)?;

    let fork_request = ForkRequest {
        tool_name: tool_name.to_string(),
        fork_method: Some(fork_method),
        codex_auto_trust,
        provider_session_id: source_provider_id.clone(),
        parent_csa_session_id: source_csa_id.clone(),
        parent_session_dir: session_dir.clone(),
        working_dir: project_root.to_path_buf(),
        timeout: std::time::Duration::from_secs(60),
    };

    let fork_info = TransportFactory::fork_session(&fork_request).await;

    if !fork_info.success {
        let notes = fork_info.notes.unwrap_or_default();
        anyhow::bail!(
            "Fork failed for session {} ({:?}): {}",
            source_csa_id,
            fork_info.method,
            notes
        );
    }

    info!(
        source = %source_csa_id,
        method = ?fork_info.method,
        new_provider_session = ?fork_info.new_session_id,
        notes = ?fork_info.notes,
        "Session fork completed"
    );

    // For soft fork, we need to read the context summary to prepend to the prompt
    let context_prefix = if matches!(fork_info.method, ForkMethod::Soft) {
        match csa_session::soft_fork_session(&session_dir, &source_csa_id) {
            Ok(ctx) => Some(ctx.context_summary),
            Err(e) => {
                warn!("Soft fork context extraction failed (non-fatal): {e}");
                None
            }
        }
    } else {
        None
    };

    Ok(ForkResolution {
        provider_session_id: fork_info.new_session_id,
        context_prefix,
        source_session_id: source_csa_id,
        source_provider_session_id: source_provider_id,
    })
}

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
    let idle_timeout_seconds = if no_idle_timeout {
        info!("Idle timeout disabled via --no-idle-timeout");
        u64::MAX
    } else {
        pipeline::resolve_idle_timeout_seconds(config.as_ref(), idle_timeout)
    };

    // 7. Resolve initial tool based on strategy
    let mut heterogeneous_runtime_fallback_candidates: Vec<ToolName> = Vec::new();
    let (initial_tool, resolved_model_spec, resolved_model) = match &strategy {
        ToolSelectionStrategy::Explicit(t) => resolve_tool_and_model(
            Some(*t),
            model_spec.as_deref(),
            model.as_deref(),
            config.as_ref(),
            &project_root,
            force,
            force_override_user_config,
        )?,
        ToolSelectionStrategy::AnyAvailable => resolve_tool_and_model(
            None,
            model_spec.as_deref(),
            model.as_deref(),
            config.as_ref(),
            &project_root,
            force,
            force_override_user_config,
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
                            force,
                            force_override_user_config,
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
                            force,
                            force_override_user_config,
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
                    force,
                    force_override_user_config,
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
                        force,
                        force_override_user_config,
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
                    force,
                    force_override_user_config,
                )?
            }
        }
    };

    let resolved_tool = initial_tool;

    // Auto seed fork: if no explicit fork/session requested, try to fork from a warm seed
    let mut is_auto_seed_fork = false;
    let (next_is_fork, next_session_arg) = if !is_fork && session_arg.is_none() && !ephemeral {
        let auto_seed_enabled = config
            .as_ref()
            .map(|c| c.session.auto_seed_fork)
            .unwrap_or(true);
        if auto_seed_enabled {
            let seed_max_age = config
                .as_ref()
                .map(|c| c.session.seed_max_age_secs)
                .unwrap_or(86400);
            let current_git_head = csa_session::detect_git_head(&project_root);
            let needs_native_fork = matches!(
                TransportFactory::fork_method_for_tool(resolved_tool.as_str()),
                ForkMethod::Native,
            );
            let seed_result = if needs_native_fork {
                csa_scheduler::find_seed_session_for_native_fork(
                    &project_root,
                    resolved_tool.as_str(),
                    seed_max_age,
                    current_git_head.as_deref(),
                )
            } else {
                csa_scheduler::find_seed_session(
                    &project_root,
                    resolved_tool.as_str(),
                    seed_max_age,
                    current_git_head.as_deref(),
                )
            };
            match seed_result {
                Ok(Some(seed)) => {
                    info!(
                        seed_session = %seed.session_id,
                        tool = %seed.tool_name,
                        "Auto fork-from-seed: warm session found"
                    );
                    is_auto_seed_fork = true;
                    (true, Some(seed.session_id))
                }
                Ok(None) => {
                    debug!("No seed session available, cold start");
                    (is_fork, session_arg)
                }
                Err(e) => {
                    debug!(error = %e, "Seed session lookup failed, falling back to cold start");
                    (is_fork, session_arg)
                }
            }
        } else {
            (is_fork, session_arg)
        }
    } else {
        (is_fork, session_arg)
    };
    is_fork = next_is_fork;
    session_arg = next_session_arg;

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
            if let Some(mut held_slot) = _slot_guard.take() {
                held_slot.release_slot()?;
                info!(
                    tool = %tool_name_str,
                    "Released parent slot before fork-call child execution"
                );
            }

            let child_slot = if wait {
                let timeout = std::time::Duration::from_secs(resolve_slot_wait_timeout_seconds(
                    config.as_ref(),
                ));
                acquire_slot_blocking(
                    &slots_dir,
                    tool_name_str,
                    max_concurrent,
                    timeout,
                    session_arg.as_deref(),
                )?
            } else {
                match try_acquire_slot(
                    &slots_dir,
                    tool_name_str,
                    max_concurrent,
                    session_arg.as_deref(),
                )? {
                    SlotAcquireResult::Acquired(slot) => slot,
                    SlotAcquireResult::Exhausted(status) => {
                        let all_tools = global_config.all_tool_slots();
                        let all_tools_ref: Vec<(&str, u32)> =
                            all_tools.iter().map(|(n, m)| (*n, *m)).collect();
                        let all_usage = slot_usage(&slots_dir, &all_tools_ref);
                        let diag_msg = format_slot_diagnostic(tool_name_str, &status, &all_usage);
                        eprintln!("{}", diag_msg);
                        return Ok(1);
                    }
                }
            };

            info!(
                tool = %tool_name_str,
                slot = child_slot.slot_index(),
                "Acquired child slot for fork-call execution"
            );
            _slot_guard = Some(child_slot);
        }

        // Resolve fork lazily: only after slot acquisition confirms we will proceed.
        // This prevents orphaning transport-level forks when pre-run checks fail.
        if is_fork && fork_resolution.is_none() {
            if let Some(ref source_id) = session_arg {
                let codex_auto_trust = config
                    .as_ref()
                    .is_some_and(ProjectConfig::codex_auto_trust);
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
        // in tool state so that execute_with_session_and_meta can resume ACP from the
        // forked provider session on the very first execution.
        if effective_session_arg.is_none() {
            if let Some(ref fork_res) = fork_resolution {
                if let Some(ref new_provider_id) = fork_res.provider_session_id {
                    let fork_desc = description.clone().unwrap_or_else(|| {
                        format!(
                            "fork of {}",
                            fork_res
                                .source_session_id
                                .get(..8)
                                .unwrap_or(&fork_res.source_session_id)
                        )
                    });
                    let mut pre_session = csa_session::create_session(
                        &project_root,
                        Some(&fork_desc),
                        Some(&fork_res.source_session_id),
                        Some(current_tool.as_str()),
                    )?;
                    pre_session.genealogy.fork_of_session_id =
                        Some(fork_res.source_session_id.clone());
                    pre_session.genealogy.fork_provider_session_id =
                        fork_res.source_provider_session_id.clone();
                    pre_session.tools.insert(
                        current_tool.as_str().to_string(),
                        ToolState {
                            provider_session_id: Some(new_provider_id.clone()),
                            last_action_summary: String::new(),
                            last_exit_code: 0,
                            updated_at: chrono::Utc::now(),
                            token_usage: None,
                        },
                    );
                    csa_session::save_session(&pre_session)?;
                    info!(
                        session = %pre_session.meta_session_id,
                        provider_session = %new_provider_id,
                        "Pre-created session with forked provider session for ACP resume"
                    );
                    pre_created_fork_session_id = Some(pre_session.meta_session_id.clone());
                    effective_session_arg = Some(pre_session.meta_session_id.clone());
                }
            }
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

        if fork_call
            && let Some(instructions) = structured_output_instructions_for_fork_call(true)
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

            // Prefer the actually-executed session (important for forks where
            // effective_session_arg starts as None) so decide_failover evaluates
            // the fork session's context, not the parent session.
            let failover_session_ref = executed_session_id
                .as_ref()
                .or(effective_session_arg.as_ref());
            let session_state = if !ephemeral {
                failover_session_ref.and_then(|sid| {
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

    if fork_call {
        let child_session_id = executed_session_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("fork-call completed without child session id"))?;
        let (return_packet, return_packet_ref) =
            load_child_return_packet(&project_root, child_session_id)?;

        let parent_session_id = fork_call_parent_session_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("fork-call parent session is unresolved"))?;
        // Reload current state from disk to avoid clobbering concurrent parent updates.
        let mut parent_state = csa_session::load_session(&project_root, &parent_session_id)?;
        parent_state.last_return_packet = Some(return_packet_ref);
        csa_session::save_session(&parent_state)?;

        // Reacquire a slot for parent resume work after child execution.
        // This is best-effort only; return-packet persistence is the critical path.
        let parent_tool_name = current_tool.as_str();
        let parent_timeout =
            std::time::Duration::from_secs(resolve_slot_wait_timeout_seconds(config.as_ref()));
        let _parent_resume_slot = match acquire_slot_blocking(
            &slots_dir,
            parent_tool_name,
            global_config.max_concurrent(parent_tool_name),
            parent_timeout,
            Some(&parent_session_id),
        ) {
            Ok(slot) => Some(slot),
            Err(e) => {
                warn!(
                    session = %parent_session_id,
                    error = %e,
                    "Failed to reacquire parent slot during fork-call resume; continuing"
                );
                None
            }
        };

        if return_target.is_some() {
            match parent_state.phase {
                SessionPhase::Available => {
                    parent_state
                        .apply_phase_event(PhaseEvent::Resumed)
                        .map_err(anyhow::Error::msg)?;
                }
                SessionPhase::Active => {
                    debug!(
                        session = %parent_state.meta_session_id,
                        "Parent already active; skipping Resumed transition"
                    );
                }
                SessionPhase::Retired => {
                    warn!(
                        session = %parent_state.meta_session_id,
                        "Parent session is retired; skipping auto-resume"
                    );
                }
            }
        }

        csa_session::save_session(&parent_state)?;
        info!(
            parent = %parent_session_id,
            child = %child_session_id,
            status = ?return_packet.status,
            exit_code = return_packet.exit_code,
            "Stored return packet ref and completed fork-call parent resume"
        );
    }

    // Update fork genealogy on the executed session (post-execution).
    // Use the actual executed session ID instead of a global "newest" lookup.
    if let Some(ref fork_res) = fork_resolution {
        if let Some(ref sid) = executed_session_id {
            match csa_session::load_session(&project_root, sid) {
                Ok(mut session) => {
                    session.genealogy.fork_of_session_id = Some(fork_res.source_session_id.clone());
                    session.genealogy.fork_provider_session_id =
                        fork_res.source_provider_session_id.clone();
                    if session.genealogy.parent_session_id.is_none() {
                        session.genealogy.parent_session_id =
                            Some(fork_res.source_session_id.clone());
                    }
                    // For native fork: store the forked provider session ID in
                    // ToolState so future `--session` resumes can use it.
                    if let Some(ref new_provider_id) = fork_res.provider_session_id {
                        if let Some(tool_state) = session.tools.get_mut(current_tool.as_str()) {
                            tool_state.provider_session_id = Some(new_provider_id.clone());
                        }
                    }
                    if let Err(e) = csa_session::save_session(&session) {
                        warn!("Failed to update fork genealogy on session: {e}");
                    } else {
                        info!(
                            session = %session.meta_session_id,
                            fork_of = %fork_res.source_session_id,
                            "Updated session genealogy with fork fields"
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to load session for fork genealogy update: {e}");
                }
            }
        }
    }

    // Mark successful non-fork sessions as seed candidates and run LRU eviction.
    // Use the actual executed session ID to avoid tagging an unrelated newer session.
    if result.exit_code == 0 && fork_resolution.is_none() && !ephemeral {
        if let Some(ref sid) = executed_session_id {
            match csa_session::load_session(&project_root, sid) {
                Ok(mut session) => {
                    if !session.is_seed_candidate {
                        session.is_seed_candidate = true;
                        if let Err(e) = csa_session::save_session(&session) {
                            warn!("Failed to mark session as seed candidate: {e}");
                        } else {
                            info!(
                                session = %session.meta_session_id,
                                tool = %current_tool.as_str(),
                                "Marked session as seed candidate"
                            );
                        }
                    }
                }
                Err(e) => {
                    debug!(error = %e, "Failed to load session for seed marking");
                }
            }
        }

        // LRU eviction: retire excess seed sessions for this tool×project
        let max_seeds = config
            .as_ref()
            .map(|c| c.session.max_seed_sessions)
            .unwrap_or(2);
        match csa_scheduler::evict_excess_seeds(&project_root, current_tool.as_str(), max_seeds) {
            Ok(retired) if !retired.is_empty() => {
                info!(
                    count = retired.len(),
                    tool = %current_tool.as_str(),
                    "Evicted excess seed sessions"
                );
            }
            Err(e) => {
                debug!(error = %e, "Seed eviction check failed");
            }
            _ => {}
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
