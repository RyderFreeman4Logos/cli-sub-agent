use anyhow::{Context, Result};
use clap::Parser;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, warn};

mod batch;
mod cli;
mod config_cmds;
mod doctor;
mod gc;
mod mcp_server;
mod review_cmd;
mod run_helpers;
mod self_update;
mod session_cmds;
mod setup_cmds;
mod skill_cmds;
mod tiers_cmd;

use cli::{
    Cli, Commands, ConfigCommands, SessionCommands, SetupCommands, SkillCommands, TiersCommands,
};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_executor::{create_session_log_writer, Executor};
use csa_lock::acquire_lock;
use csa_lock::slot::{
    format_slot_diagnostic, slot_usage, try_acquire_slot, SlotAcquireResult, ToolSlot,
};
use csa_process::check_tool_installed;
use csa_resource::{MemoryMonitor, ResourceGuard, ResourceLimits};
use csa_session::{
    create_session, get_session_dir, load_session, resolve_session_prefix, save_session,
    TokenUsage, ToolState,
};
use run_helpers::{
    build_executor, is_compress_command, is_tool_binary_available, parse_token_usage,
    parse_tool_name, read_prompt, resolve_tool_and_model, truncate_prompt,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Read current depth from env
    let current_depth: u32 = std::env::var("CSA_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // Initialize tracing (output to stderr, initialize only once)
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let cli = Cli::parse();
    let output_format = cli.format.clone();

    match cli.command {
        Commands::Run {
            tool,
            prompt,
            session,
            last,
            description,
            parent,
            ephemeral,
            cd,
            model_spec,
            model,
            thinking,
            no_failover,
            wait,
        } => {
            let exit_code = handle_run(
                tool,
                prompt,
                session,
                last,
                description,
                parent,
                ephemeral,
                cd,
                model_spec,
                model,
                thinking,
                no_failover,
                wait,
                current_depth,
                output_format,
            )
            .await?;
            std::process::exit(exit_code);
        }
        Commands::Session { cmd } => match cmd {
            SessionCommands::List { cd, tool, tree } => {
                session_cmds::handle_session_list(cd, tool, tree, output_format)?;
            }
            SessionCommands::Compress { session, cd } => {
                session_cmds::handle_session_compress(session, cd)?;
            }
            SessionCommands::Delete { session, cd } => {
                session_cmds::handle_session_delete(session, cd)?;
            }
            SessionCommands::Clean {
                days,
                dry_run,
                tool,
                cd,
            } => {
                session_cmds::handle_session_clean(days, dry_run, tool, cd)?;
            }
            SessionCommands::Logs { session, tail, cd } => {
                session_cmds::handle_session_logs(session, tail, cd)?;
            }
        },
        Commands::Init {
            non_interactive,
            minimal,
        } => {
            config_cmds::handle_init(non_interactive, minimal)?;
        }
        Commands::Gc {
            dry_run,
            max_age_days,
        } => {
            gc::handle_gc(dry_run, max_age_days, output_format)?;
        }
        Commands::Config { cmd } => match cmd {
            ConfigCommands::Show { cd } => {
                config_cmds::handle_config_show(cd, output_format)?;
            }
            ConfigCommands::Edit { cd } => {
                config_cmds::handle_config_edit(cd)?;
            }
            ConfigCommands::Validate { cd } => {
                config_cmds::handle_config_validate(cd)?;
            }
        },
        Commands::Review(args) => {
            let exit_code = review_cmd::handle_review(args, current_depth).await?;
            std::process::exit(exit_code);
        }
        Commands::Doctor => {
            doctor::run_doctor().await?;
        }
        Commands::Batch { file, cd, dry_run } => {
            batch::handle_batch(file, cd, dry_run, current_depth).await?;
        }
        Commands::McpServer => {
            mcp_server::run_mcp_server().await?;
        }
        Commands::Skill { cmd } => match cmd {
            SkillCommands::Install { source, target } => {
                skill_cmds::handle_skill_install(source, target)?;
            }
            SkillCommands::List => {
                skill_cmds::handle_skill_list()?;
            }
        },
        Commands::Setup { cmd } => match cmd {
            SetupCommands::ClaudeCode => {
                setup_cmds::handle_setup_claude_code()?;
            }
            SetupCommands::Codex => {
                setup_cmds::handle_setup_codex()?;
            }
            SetupCommands::OpenCode => {
                setup_cmds::handle_setup_opencode()?;
            }
        },
        Commands::Tiers { cmd } => match cmd {
            TiersCommands::List { cd } => {
                tiers_cmd::handle_tiers_list(cd, output_format)?;
            }
        },
        Commands::SelfUpdate { check } => {
            self_update::handle_self_update(check)?;
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_run(
    tool: Option<ToolName>,
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
    current_depth: u32,
    output_format: OutputFormat,
) -> Result<i32> {
    // 1. Determine project root
    let project_root = determine_project_root(cd.as_deref())?;

    // 2. Resolve --last flag to session ID
    let session_arg = if last {
        let sessions = csa_session::list_sessions(&project_root, None)?;
        if sessions.is_empty() {
            anyhow::bail!("No sessions found. Run a task first to create one.");
        }
        // Sessions should be sorted by last_accessed (most recent first)
        let mut sorted_sessions = sessions;
        sorted_sessions.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
        Some(sorted_sessions[0].meta_session_id.clone())
    } else {
        session_arg
    };

    // 3. Load configs (project config + global config)
    let config = ProjectConfig::load(&project_root)?;
    let global_config = GlobalConfig::load()?;

    // 4. Check recursion depth (from config or default)
    let max_depth = config
        .as_ref()
        .map(|c| c.project.max_recursion_depth)
        .unwrap_or(5u32);

    if current_depth > max_depth {
        error!(
            "Max recursion depth ({}) exceeded. Current: {}. Do it yourself.",
            max_depth, current_depth
        );
        return Ok(1);
    }

    // 5. Read prompt
    let prompt_text = read_prompt(prompt)?;

    // 6. Resolve tool and model_spec
    let (resolved_tool, resolved_model_spec, resolved_model) = resolve_tool_and_model(
        tool.clone(),
        model_spec.as_deref(),
        model.as_deref(),
        config.as_ref(),
        &project_root,
    )?;

    // Determine max failover attempts from tier config
    let max_failover_attempts = if no_failover {
        1 // Single attempt, no failover
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

    // Resolve slots directory
    let slots_dir = GlobalConfig::slots_dir()?;

    // Failover state
    let mut current_tool = resolved_tool;
    let mut current_model_spec = resolved_model_spec;
    let mut current_model = resolved_model;
    let mut tried_tools: Vec<String> = Vec::new();
    let mut attempts = 0;

    let result = loop {
        attempts += 1;

        // 7. Build executor
        let mut executor = build_executor(
            &current_tool,
            current_model_spec.as_deref(),
            current_model.as_deref(),
            thinking.as_deref(),
        )?;

        // 7b. Inject suppress_notify from config (codex only)
        if let Some(ref cfg) = config {
            executor.set_suppress_notify(cfg.should_suppress_codex_notify());
        }

        // 8. Check tool is installed
        if let Err(e) = check_tool_installed(executor.executable_name()).await {
            error!(
                "Tool '{}' is not installed.\n\n{}\n\nOr disable it in .csa/config.toml:\n  [tools.{}]\n  enabled = false",
                executor.tool_name(),
                executor.install_hint(),
                executor.tool_name()
            );
            anyhow::bail!("{}", e);
        }

        // 9. Check tool is enabled in config
        if let Some(ref cfg) = config {
            if !cfg.is_tool_enabled(executor.tool_name()) {
                error!(
                    "Tool '{}' is disabled in project config",
                    executor.tool_name()
                );
                return Ok(1);
            }
        }

        // 10. Acquire global slot
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
                // All slots occupied. Try failover to another tool or wait.
                let all_tools = global_config.all_tool_slots();
                let all_tools_ref: Vec<(&str, u32)> =
                    all_tools.iter().map(|(n, m)| (*n, *m)).collect();
                let all_usage = slot_usage(&slots_dir, &all_tools_ref);
                let diag_msg = format_slot_diagnostic(tool_name_str, &status, &all_usage);

                // Check if we can failover to another tool with free slots
                if !no_failover && attempts < max_failover_attempts {
                    let free_alt = all_usage.iter().find(|s| {
                        s.tool_name != tool_name_str
                            && s.free() > 0
                            && !tried_tools.contains(&s.tool_name)
                            && config
                                .as_ref()
                                .map(|c| c.is_tool_enabled(&s.tool_name))
                                .unwrap_or(true)
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

                // No failover possible. Wait or abort.
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

        // Get env vars for this tool from global config
        let extra_env = global_config.env_vars(tool_name_str).cloned();

        // 11. Execute
        let exec_result = if ephemeral {
            // Ephemeral: use temp directory
            let temp_dir = TempDir::new()?;
            info!("Ephemeral session in: {:?}", temp_dir.path());
            executor
                .execute_in(&prompt_text, temp_dir.path(), extra_env.as_ref())
                .await?
        } else {
            // Persistent session
            match execute_with_session(
                &executor,
                &current_tool,
                &prompt_text,
                session_arg.clone(),
                description.clone(),
                parent.clone(),
                &project_root,
                config.as_ref(),
                extra_env.as_ref(),
            )
            .await
            {
                Ok(result) => result,
                Err(e) => {
                    // BUG-13: Check if this is a lock error and format as JSON if needed
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
                    // Not a lock error or text format - propagate normally
                    return Err(e);
                }
            }
        };

        // 12. Check for 429 rate limit and attempt failover
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

            // Don't exceed max attempts
            if attempts >= max_failover_attempts {
                warn!(
                    "Max failover attempts ({}) reached, returning error",
                    max_failover_attempts
                );
                break exec_result;
            }

            tried_tools.push(tool_name_str.to_string());

            // Load current session state for failover decision
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

            // Determine if task needs edit capability
            let needs_edit = config
                .as_ref()
                .is_some_and(|cfg| cfg.can_tool_edit_existing(tool_name_str));

            if let Some(ref cfg) = config {
                let action = csa_scheduler::decide_failover(
                    tool_name_str,
                    "default",
                    needs_edit,
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
                // No config → can't failover
                break exec_result;
            }
        } else {
            // No rate limit → return result
            break exec_result;
        }
    };

    // 13. Print result
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_with_session(
    executor: &Executor,
    tool: &ToolName,
    prompt: &str,
    session_arg: Option<String>,
    description: Option<String>,
    parent: Option<String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    extra_env: Option<&std::collections::HashMap<String, String>>,
) -> Result<csa_process::ExecutionResult> {
    // Check for parent session violation: a child process must not operate on its own session
    if let Some(ref session_id) = session_arg {
        if let Ok(env_session) = std::env::var("CSA_SESSION_ID") {
            if env_session == *session_id {
                return Err(csa_core::error::AppError::ParentSessionViolation.into());
            }
        }
    }

    // Resolve or create session
    let mut session = if let Some(ref session_id) = session_arg {
        let sessions_dir = csa_session::get_session_root(project_root)?.join("sessions");
        let resolved_id = resolve_session_prefix(&sessions_dir, session_id)?;
        load_session(project_root, &resolved_id)?
    } else {
        let parent_id = parent.or_else(|| std::env::var("CSA_SESSION_ID").ok());
        create_session(project_root, description.as_deref(), parent_id.as_deref())?
    };

    let session_dir = get_session_dir(project_root, &session.meta_session_id)?;

    // Create session log writer
    let (_log_writer, _log_guard) =
        create_session_log_writer(&session_dir).context("Failed to create session log writer")?;

    // Acquire lock with truncated prompt as reason
    let lock_reason = truncate_prompt(prompt, 80);
    let _lock =
        acquire_lock(&session_dir, executor.tool_name(), &lock_reason).with_context(|| {
            format!(
                "Failed to acquire lock for session {}",
                session.meta_session_id
            )
        })?;

    // Resource guard
    let mut resource_guard = if let Some(cfg) = config {
        let limits = ResourceLimits {
            min_free_memory_mb: cfg.resources.min_free_memory_mb,
            min_free_swap_mb: cfg.resources.min_free_swap_mb,
            initial_estimates: cfg.resources.initial_estimates.clone(),
        };
        // Stats stored at project state level, not per-session
        let project_state_dir = csa_session::get_session_root(project_root)?;
        let stats_path = project_state_dir.join("usage_stats.toml");
        Some(ResourceGuard::new(limits, &stats_path))
    } else {
        None
    };

    // Check resource availability
    if let Some(ref mut guard) = resource_guard {
        guard.check_availability(executor.tool_name())?;
    }

    info!("Executing in session: {}", session.meta_session_id);

    // Apply restrictions if configured
    let can_edit = config.map_or(true, |cfg| cfg.can_tool_edit_existing(executor.tool_name()));
    let effective_prompt = if !can_edit {
        info!(tool = %executor.tool_name(), "Applying edit restriction: tool cannot modify existing files");
        executor.apply_restrictions(prompt, false)
    } else {
        prompt.to_string()
    };

    // Build command
    let tool_state = session.tools.get(executor.tool_name()).cloned();
    let cmd = executor.build_command(&effective_prompt, tool_state.as_ref(), &session, extra_env);

    // Spawn child process
    let child = csa_process::spawn_tool(cmd)
        .await
        .context("Failed to spawn tool process")?;

    // Get child PID and start memory monitor
    let child_pid = child.id().context("Failed to get child process PID")?;
    let monitor = MemoryMonitor::start(child_pid);

    // Set up signal handlers for SIGTERM and SIGINT
    let mut sigterm =
        signal(SignalKind::terminate()).context("Failed to install SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("Failed to install SIGINT handler")?;

    // Wait for either child completion or signal
    let wait_future = csa_process::wait_and_capture(child);
    tokio::pin!(wait_future);

    let result = tokio::select! {
        result = &mut wait_future => {
            result.context("Failed to wait for tool process")?
        }
        _ = sigterm.recv() => {
            info!("Received SIGTERM, forwarding to child process group");
            // Forward SIGTERM to the child's process group (negative PID)
            // SAFETY: kill() is async-signal-safe. We use the negative of child_pid
            // to target the entire process group created by setsid().
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child_pid as i32), libc::SIGTERM);
            }
            // Wait for child to exit after signal
            wait_future.await.context("Failed to wait for tool process after SIGTERM")?
        }
        _ = sigint.recv() => {
            info!("Received SIGINT, forwarding to child process group");
            // Forward SIGINT to the child's process group
            // SAFETY: Same as SIGTERM handler above
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child_pid as i32), libc::SIGINT);
            }
            // Wait for child to exit after signal
            wait_future.await.context("Failed to wait for tool process after SIGINT")?
        }
    };

    // Stop memory monitor and record usage
    let peak_memory_mb = monitor.stop().await;
    if let Some(ref mut guard) = resource_guard {
        guard.record_usage(executor.tool_name(), peak_memory_mb);
    }

    // Extract provider session ID from output
    let provider_session_id = csa_executor::extract_session_id(tool, &result.output);

    // Parse token usage from output (best-effort)
    let token_usage = parse_token_usage(&result.output);

    // Update session state
    session
        .tools
        .entry(executor.tool_name().to_string())
        .and_modify(|t| {
            // Only update provider_session_id if extraction succeeded
            if let Some(ref session_id) = provider_session_id {
                t.provider_session_id = Some(session_id.clone());
            }
            t.last_action_summary = result.summary.clone();
            t.last_exit_code = result.exit_code;
            t.updated_at = chrono::Utc::now();

            // Update token usage if parsed successfully
            if let Some(ref usage) = token_usage {
                t.token_usage = Some(usage.clone());
            }
        })
        .or_insert_with(|| ToolState {
            provider_session_id,
            last_action_summary: result.summary.clone(),
            last_exit_code: result.exit_code,
            updated_at: chrono::Utc::now(),
            token_usage: token_usage.clone(),
        });
    session.last_accessed = chrono::Utc::now();

    // Detect compress/compact commands: mark session as Available for reuse
    if result.exit_code == 0 && is_compress_command(prompt) {
        session.context_status.is_compacted = true;
        session.context_status.last_compacted_at = Some(chrono::Utc::now());
        session.phase = csa_session::SessionPhase::Available;
        info!(
            session = %session.meta_session_id,
            "Session compacted and marked Available for reuse"
        );
    }

    // Update cumulative token usage if we got new tokens
    if let Some(new_usage) = token_usage {
        let cumulative = session
            .total_token_usage
            .get_or_insert(TokenUsage::default());
        cumulative.input_tokens =
            Some(cumulative.input_tokens.unwrap_or(0) + new_usage.input_tokens.unwrap_or(0));
        cumulative.output_tokens =
            Some(cumulative.output_tokens.unwrap_or(0) + new_usage.output_tokens.unwrap_or(0));
        cumulative.total_tokens =
            Some(cumulative.total_tokens.unwrap_or(0) + new_usage.total_tokens.unwrap_or(0));
        cumulative.estimated_cost_usd = Some(
            cumulative.estimated_cost_usd.unwrap_or(0.0)
                + new_usage.estimated_cost_usd.unwrap_or(0.0),
        );
    }

    // Save session
    save_session(&session)?;

    Ok(result)
}

pub(crate) fn determine_project_root(cd: Option<&str>) -> Result<PathBuf> {
    let path = if let Some(cd_path) = cd {
        PathBuf::from(cd_path)
    } else {
        std::env::current_dir()?
    };

    Ok(path.canonicalize()?)
}
