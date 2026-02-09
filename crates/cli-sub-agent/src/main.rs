use anyhow::Result;
use clap::Parser;
use tempfile::TempDir;
use tracing::{info, warn};

mod batch;
mod cli;
mod config_cmds;
mod debate_cmd;
mod doctor;
mod gc;
mod mcp_server;
mod pipeline;
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
use csa_config::GlobalConfig;
use csa_core::types::{OutputFormat, ToolArg, ToolSelectionStrategy};
use csa_lock::slot::{
    format_slot_diagnostic, slot_usage, try_acquire_slot, SlotAcquireResult, ToolSlot,
};
use csa_session::{load_session, resolve_session_prefix};
use run_helpers::{
    infer_task_edit_requirement, is_tool_binary_available, parse_tool_name, read_prompt,
    resolve_tool_and_model,
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
        Commands::Debate(args) => {
            let exit_code = debate_cmd::handle_debate(args, current_depth).await?;
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
    tool: Option<ToolArg>,
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
    let project_root = pipeline::determine_project_root(cd.as_deref())?;

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

    // 3. Load configs and validate recursion depth
    let Some((config, global_config)) = pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };

    // 5. Read prompt
    let prompt_text = read_prompt(prompt)?;

    // 6. Convert ToolArg to ToolSelectionStrategy
    let strategy = tool.unwrap_or(ToolArg::Auto).into_strategy();

    // 7. Resolve initial tool based on strategy
    let (initial_tool, resolved_model_spec, resolved_model) = match &strategy {
        ToolSelectionStrategy::Explicit(t) => {
            // Explicit tool from CLI — still apply alias resolution
            resolve_tool_and_model(
                Some(*t),
                model_spec.as_deref(),
                model.as_deref(),
                config.as_ref(),
                &project_root,
            )?
        }
        ToolSelectionStrategy::AnyAvailable => {
            // Use tier-based selection (current behavior)
            resolve_tool_and_model(
                None,
                model_spec.as_deref(),
                model.as_deref(),
                config.as_ref(),
                &project_root,
            )?
        }
        ToolSelectionStrategy::HeterogeneousStrict => {
            // Get parent tool from environment
            let parent_tool_name = std::env::var("CSA_TOOL")
                .or_else(|_| std::env::var("CSA_PARENT_TOOL"))
                .ok();

            if let Some(parent_str) = parent_tool_name.as_deref() {
                // Have parent context, resolve heterogeneous tool
                let parent_tool = parse_tool_name(parent_str)?;
                let enabled_tools = if let Some(ref cfg) = config {
                    csa_config::global::all_known_tools()
                        .iter()
                        .filter(|t| cfg.is_tool_enabled(t.as_str()))
                        .copied()
                        .collect::<Vec<_>>()
                } else {
                    csa_config::global::all_known_tools().to_vec()
                };

                match csa_config::global::select_heterogeneous_tool(&parent_tool, &enabled_tools) {
                    Some(tool) => {
                        // Resolve model/model-spec for the selected tool (preserves --model/--model-spec flags)
                        resolve_tool_and_model(
                            Some(tool),
                            model_spec.as_deref(),
                            model.as_deref(),
                            config.as_ref(),
                            &project_root,
                        )?
                    }
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
                // No parent context, fall back to AnyAvailable with warning
                warn!("HeterogeneousStrict requested but no parent tool context found. Falling back to AnyAvailable.");
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

        // 7. Build executor and validate tool
        let executor = pipeline::build_and_validate_executor(
            &current_tool,
            current_model_spec.as_deref(),
            current_model.as_deref(),
            thinking.as_deref(),
            config.as_ref(),
        )
        .await?;

        // 8. Acquire global slot
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

        // 9. Execute
        let exec_result = if ephemeral {
            // Ephemeral: use temp directory
            let temp_dir = TempDir::new()?;
            info!("Ephemeral session in: {:?}", temp_dir.path());
            executor
                .execute_in(&prompt_text, temp_dir.path(), extra_env.as_ref())
                .await?
        } else {
            // Persistent session
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

        // 10. Check for 429 rate limit and attempt failover
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

            // Infer task edit requirement from prompt when explicit.
            // If ambiguous, keep conservative behavior by falling back to the failed tool's
            // configured edit capability.
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
                // No config → can't failover
                break exec_result;
            }
        } else {
            // No rate limit → return result
            break exec_result;
        }
    };

    // 11. Print result
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
