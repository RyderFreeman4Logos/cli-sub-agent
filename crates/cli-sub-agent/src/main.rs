use std::io::Write;

use anyhow::Result;
use clap::Parser;

mod audit;
mod audit_cmds;
mod batch;
mod claude_sub_agent_cmd;
mod cli;
mod config_cmds;
mod debate_cmd;
mod debate_cmd_output;
mod debate_cmd_resolve;
mod debate_errors;
mod doctor;
mod edit_restriction_guard;
mod error_hints;
mod eval_cmd;
mod gc;
mod mcp_hub;
mod mcp_server;
mod memory_capture;
mod memory_cmd;
mod pattern_resolver;
mod pipeline;
mod pipeline_env;
mod pipeline_execute;
mod pipeline_handoff;
mod pipeline_post_exec;
mod pipeline_project_key;
mod pipeline_sandbox;
mod pipeline_transcript;
mod plan_cmd;
mod plan_condition;
mod plan_display;
mod process_tree;
mod review_cmd;
mod review_consensus;
mod review_context;
mod review_routing;
mod run_cmd;
mod run_cmd_daemon;
mod run_cmd_fork;
mod run_cmd_post;
mod run_cmd_tool_selection;
mod run_helpers;
mod self_update;
mod session_cmds;
mod session_cmds_daemon;
mod session_cmds_result;
mod session_dispatch;
mod session_guard;
mod setup_cmds;
mod skill_cmds;
mod skill_resolver;
mod tiers_cmd;
mod todo_cmd;
mod todo_ref_cmd;

#[cfg(test)]
mod sa_mode_tests;

#[cfg(test)]
mod test_env_lock;

use cli::{
    Cli, Commands, ConfigCommands, McpHubCommands, PlanCommands, SetupCommands, SkillCommands,
    TiersCommands, TodoCommands, TodoRefCommands, handle_tokuin, handle_xurl,
    validate_command_args,
};
use csa_core::types::OutputFormat;

mod migrate_cmd;

const SA_MODE_REQUIRED_ERROR_PREFIX: &str = "--sa-mode true|false is required for root callers on execution commands.\n\
     Hint: add --sa-mode false for interactive use, or --sa-mode true for autonomous workflows";
const CSA_INTERNAL_INVOCATION_ENV: &str = "CSA_INTERNAL_INVOCATION";

fn command_name_for_sa_mode(command: &Commands) -> Option<&'static str> {
    match command {
        Commands::Run { .. } => Some("run"),
        Commands::Review(_) => Some("review"),
        Commands::Debate(_) => Some("debate"),
        Commands::Batch { .. } => Some("batch"),
        Commands::Plan {
            cmd: PlanCommands::Run { .. },
        } => Some("plan run"),
        Commands::ClaudeSubAgent(_) => Some("claude-sub-agent"),
        _ => None,
    }
}

fn command_sa_mode_arg(command: &Commands) -> Option<Option<bool>> {
    match command {
        Commands::Run { sa_mode, .. } => Some(*sa_mode),
        Commands::Review(args) => Some(args.sa_mode),
        Commands::Debate(args) => Some(args.sa_mode),
        Commands::Batch { sa_mode, .. } => Some(*sa_mode),
        Commands::Plan {
            cmd: PlanCommands::Run { sa_mode, .. },
        } => Some(*sa_mode),
        Commands::ClaudeSubAgent(args) => Some(args.sa_mode),
        _ => None,
    }
}

fn is_internal_sa_invocation(current_depth: u32) -> bool {
    if current_depth == 0 {
        return false;
    }

    std::env::var(CSA_INTERNAL_INVOCATION_ENV)
        .ok()
        .map(|raw| {
            let normalized = raw.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

pub(crate) fn validate_sa_mode(command: &Commands, current_depth: u32) -> anyhow::Result<bool> {
    let Some(sa_mode_arg) = command_sa_mode_arg(command) else {
        return Ok(false);
    };

    if sa_mode_arg.is_none() && !is_internal_sa_invocation(current_depth) {
        let command_name = command_name_for_sa_mode(command).unwrap_or("execution command");
        anyhow::bail!("{SA_MODE_REQUIRED_ERROR_PREFIX}: command `{command_name}`");
    }

    Ok(sa_mode_arg.unwrap_or(false))
}

/// Resolve the effective minimum timeout from project and global configs.
///
/// Priority: project `[execution].min_timeout_seconds` > global > compile-time default.
/// Config loading errors are silently ignored (fall back to compile-time default).
fn resolve_effective_min_timeout() -> u64 {
    let compile_default = csa_config::ExecutionConfig::default_min_timeout();

    // Try to load project config (merged with user-level).
    // This is the same merged config that pipeline uses, so project overrides global
    // via the standard TOML deep-merge path.
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(Some(config)) = csa_config::ProjectConfig::load(&cwd)
        && !config.execution.is_default()
    {
        return config.execution.min_timeout_seconds;
    }

    // Fall back to global config.
    if let Ok(global) = csa_config::GlobalConfig::load()
        && !global.execution.is_default()
    {
        return global.execution.min_timeout_seconds;
    }

    compile_default
}

fn apply_sa_mode_prompt_guard(command: &Commands, current_depth: u32) -> anyhow::Result<()> {
    if command_sa_mode_arg(command).is_none() {
        return Ok(());
    }

    let sa_mode_enabled = validate_sa_mode(command, current_depth)?;
    let value = if sa_mode_enabled { "true" } else { "false" };

    // SAFETY: process-level env updated once during startup before async work begins.
    unsafe {
        std::env::set_var(
            crate::pipeline::prompt_guard::PROMPT_GUARD_CALLER_INJECTION_ENV,
            value,
        )
    };

    Ok(())
}

#[tokio::main]

async fn main() {
    if let Err(err) = run().await {
        eprintln!("Error: {err}");
        if let Some(hint) = error_hints::suggest_fix(&err) {
            eprintln!();
            eprintln!("{hint}");
        }
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
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
    let output_format = cli.format;
    let command = cli.command;

    // Resolve effective min_timeout_seconds from configs (project overrides global).
    // This is a lightweight load; config errors are ignored (fall back to compile-time default).
    let min_timeout = resolve_effective_min_timeout();

    if let Err(err) = validate_command_args(&command, min_timeout) {
        err.exit();
    }

    apply_sa_mode_prompt_guard(&command, current_depth)?;

    // Check weave.lock version alignment (non-fatal).
    if let Ok(cwd) = std::env::current_dir() {
        let registry = csa_config::default_registry();
        match csa_config::check_version(
            &cwd,
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_VERSION"),
            &registry,
        ) {
            Ok(csa_config::VersionCheckResult::MigrationNeeded { pending_count }) => {
                eprintln!(
                    "WARNING: weave.lock is outdated ({pending_count} pending migration(s)). \
                     Run `csa migrate` to update."
                );
            }
            Ok(csa_config::VersionCheckResult::AutoUpdated) => {
                tracing::debug!("weave.lock auto-updated to match binary version");
            }
            Ok(csa_config::VersionCheckResult::BinaryOlder {
                lock_csa_version,
                binary_csa_version,
            }) => {
                eprintln!(
                    "WARNING: running older csa binary ({binary_csa_version}) than weave.lock ({lock_csa_version}); lockfile unchanged."
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::debug!("weave.lock version check failed: {e:#}");
            }
        }
    }

    // Auto weave upgrade (if configured via [execution] auto_weave_upgrade = true).
    // ProjectConfig::load already deep-merges global config, so only fall back to
    // raw GlobalConfig when no merged config exists at all.
    // Guard: only run when weave.lock exists (skip non-weave directories).
    {
        let has_weave_lock = std::env::current_dir()
            .map(|cwd| cwd.join("weave.lock").exists())
            .unwrap_or(false);

        let auto_upgrade = has_weave_lock
            && std::env::current_dir()
                .ok()
                .and_then(|cwd| csa_config::ProjectConfig::load(&cwd).ok().flatten())
                .map(|cfg| cfg.execution.auto_weave_upgrade)
                .unwrap_or_else(|| {
                    csa_config::GlobalConfig::load()
                        .map(|g| g.execution.auto_weave_upgrade)
                        .unwrap_or(false)
                });

        if auto_upgrade {
            use std::time::Duration;

            let mut success = false;
            let mut delay = Duration::from_secs(1);

            for attempt in 0..3 {
                let result = tokio::process::Command::new("weave")
                    .arg("upgrade")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await;

                match result {
                    Ok(status) if status.success() => {
                        success = true;
                        break;
                    }
                    Ok(status) => {
                        if attempt < 2 {
                            tracing::debug!(
                                "weave upgrade failed (exit {}), retrying in {:?}...",
                                status.code().unwrap_or(-1),
                                delay
                            );
                            tokio::time::sleep(delay).await;
                            delay *= 2;
                        }
                    }
                    Err(e) => {
                        if attempt < 2 {
                            tracing::debug!(
                                "weave upgrade failed ({}), retrying in {:?}...",
                                e,
                                delay
                            );
                            tokio::time::sleep(delay).await;
                            delay *= 2;
                        }
                    }
                }
            }

            if !success {
                anyhow::bail!(
                    "auto weave upgrade failed after 3 attempts. \
                     Disable with [execution] auto_weave_upgrade = false"
                );
            }
        }
    }

    let legacy_xdg_paths = csa_config::paths::legacy_paths_requiring_migration();
    if !legacy_xdg_paths.is_empty() {
        for path in &legacy_xdg_paths {
            tracing::debug!(
                label = path.label,
                legacy = %path.legacy_path.display(),
                new = %path.new_path.display(),
                "legacy XDG path detected, auto-migrating"
            );
        }
        match csa_config::migrate::run_xdg_migration() {
            Ok(()) => {
                tracing::debug!(
                    "auto-migrated {} legacy XDG path(s)",
                    legacy_xdg_paths.len()
                );
            }
            Err(e) => {
                eprintln!(
                    "WARNING: failed to auto-migrate legacy XDG paths: {e:#}. Run `csa migrate` manually."
                );
            }
        }
    }

    match command {
        Commands::Run {
            tool,
            skill,
            sa_mode: _,
            prompt,
            prompt_file,
            session,
            last,
            fork_from,
            fork_last,
            description,
            fork_call,
            return_to,
            parent,
            ephemeral,
            cd,
            model_spec,
            model,
            thinking,
            force,
            force_override_user_config,
            no_failover,
            wait,
            idle_timeout,
            initial_response_timeout,
            timeout,
            no_idle_timeout,
            no_memory,
            memory_query,
            stream_stdout,
            no_stream_stdout,
            spec: _spec,
            tier,
            force_ignore_tier_setting,
            no_fs_sandbox,
            extra_writable,
            daemon,
            daemon_child,
            session_id,
        } => {
            // Daemon spawn: when --daemon is set and not already the child, fork and exit.
            if daemon && !daemon_child {
                if let Some(ref _id) = session_id {
                    anyhow::bail!("--session-id is an internal flag and must not be used directly");
                }
                // spawn_and_exit() calls process::exit(0) on success — never returns.
                run_cmd_daemon::spawn_and_exit(cd.as_deref())?;
            }

            // Daemon child: propagate pre-assigned session ID via env so the
            // pipeline's create_session reuses it (same directory as spool files).
            if let Some(ref sid) = session_id {
                // SAFETY: daemon child is a single-threaded process at this point.
                unsafe { std::env::set_var("CSA_DAEMON_SESSION_ID", sid) };
            }

            // Daemon child path: continue with normal run logic.
            // --stream-stdout forces streaming; --no-stream-stdout forces buffering;
            // default: stream for Text output in all contexts.
            let stream_mode = if no_stream_stdout {
                csa_process::StreamMode::BufferOnly
            } else if stream_stdout || matches!(output_format, OutputFormat::Text) {
                csa_process::StreamMode::TeeToStderr
            } else {
                csa_process::StreamMode::BufferOnly
            };

            let exit_code = run_cmd::handle_run(
                tool,
                skill,
                prompt,
                prompt_file,
                session,
                last,
                fork_from,
                fork_last,
                description,
                fork_call,
                return_to,
                parent,
                ephemeral,
                cd,
                model_spec,
                model,
                thinking,
                force,
                force_override_user_config,
                no_failover,
                wait,
                idle_timeout,
                initial_response_timeout,
                timeout,
                no_idle_timeout,
                no_memory,
                memory_query,
                current_depth,
                output_format,
                stream_mode,
                tier,
                force_ignore_tier_setting,
                no_fs_sandbox,
                extra_writable,
            )
            .await?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(exit_code);
        }
        Commands::Session { cmd } => {
            session_dispatch::dispatch(cmd, output_format)?;
        }
        Commands::Audit { command } => {
            audit_cmds::handle_audit(command)?;
        }
        Commands::Init {
            non_interactive,
            full,
            template,
        } => {
            config_cmds::handle_init(non_interactive, full, template)?;
        }
        Commands::Gc {
            dry_run,
            max_age_days,
            global,
        } => {
            if global {
                gc::handle_gc_global(dry_run, max_age_days, output_format)?;
            } else {
                gc::handle_gc(dry_run, max_age_days, output_format)?;
            }
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
            ConfigCommands::Get {
                key,
                default,
                project,
                global,
                cd,
            } => {
                config_cmds::handle_config_get(key, default, project, global, cd)?;
            }
        },
        Commands::Memory { command } => {
            memory_cmd::handle_memory_command(command).await?;
        }
        Commands::Review(args) => {
            let exit_code = review_cmd::handle_review(args, current_depth).await?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(exit_code);
        }
        Commands::Debate(args) => {
            let exit_code = debate_cmd::handle_debate(args, current_depth, output_format).await?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(exit_code);
        }
        Commands::Eval {
            passive: _,
            project,
            days,
            json,
        } => {
            eval_cmd::handle_eval(project, days, json)?;
        }
        Commands::Doctor => {
            doctor::run_doctor(output_format).await?;
        }
        Commands::Batch {
            file,
            sa_mode: _,
            cd,
            dry_run,
        } => {
            batch::handle_batch(file, cd, dry_run, current_depth).await?;
        }
        Commands::McpServer => {
            mcp_server::run_mcp_server().await?;
        }
        Commands::McpHub { cmd } => match cmd {
            McpHubCommands::Serve {
                background,
                foreground,
                socket,
                http_bind,
                http_port,
                systemd_activation,
            } => {
                mcp_hub::handle_serve_command(
                    background,
                    foreground,
                    socket,
                    http_bind,
                    http_port,
                    systemd_activation,
                )
                .await?;
            }
            McpHubCommands::Status { socket } => {
                mcp_hub::handle_status_command(socket).await?;
            }
            McpHubCommands::Stop { socket } => {
                mcp_hub::handle_stop_command(socket).await?;
            }
            McpHubCommands::GenSkill { socket } => {
                mcp_hub::handle_gen_skill_command(socket).await?;
            }
        },
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
        Commands::Todo { cmd } => match cmd {
            TodoCommands::Create {
                title,
                branch,
                no_branch,
                language,
                cd,
            } => {
                todo_cmd::handle_create(title, branch, no_branch, language, cd, output_format)?;
            }
            TodoCommands::Save {
                timestamp,
                message,
                cd,
            } => {
                todo_cmd::handle_save(timestamp, message, cd)?;
            }
            TodoCommands::Diff {
                timestamp,
                revision,
                from,
                to,
                cd,
            } => {
                todo_cmd::handle_diff(timestamp, revision, from, to, cd)?;
            }
            TodoCommands::History { timestamp, cd } => {
                todo_cmd::handle_history(timestamp, cd)?;
            }
            TodoCommands::List { status, cd } => {
                todo_cmd::handle_list(status, cd, output_format)?;
            }
            TodoCommands::Find { branch, status, cd } => {
                todo_cmd::handle_find(branch, status, cd, output_format)?;
            }
            TodoCommands::Show {
                timestamp,
                version,
                path,
                spec,
                refs,
                cd,
            } => {
                todo_cmd::handle_show(timestamp, version, path, spec, refs, cd)?;
            }
            TodoCommands::Status {
                timestamp,
                status,
                cd,
            } => {
                todo_cmd::handle_status(timestamp, status, cd)?;
            }
            TodoCommands::Dag {
                timestamp,
                format,
                cd,
            } => {
                todo_cmd::handle_dag(timestamp, format, cd)?;
            }
            TodoCommands::Ref { cmd } => match cmd {
                TodoRefCommands::List {
                    timestamp,
                    tokens,
                    json,
                    cd,
                } => {
                    todo_cmd::handle_ref_list(timestamp, tokens, json, cd)?;
                }
                TodoRefCommands::Show {
                    timestamp,
                    name,
                    max_tokens,
                    cd,
                } => {
                    todo_cmd::handle_ref_show(timestamp, name, max_tokens, cd)?;
                }
                TodoRefCommands::Add {
                    timestamp,
                    name,
                    content,
                    file,
                    cd,
                } => {
                    todo_cmd::handle_ref_add(timestamp, name, content, file, cd)?;
                }
                TodoRefCommands::ImportTranscript {
                    timestamp,
                    tool,
                    session,
                    name,
                    cd,
                } => {
                    todo_cmd::handle_ref_import_transcript(timestamp, tool, session, name, cd)?;
                }
            },
        },
        Commands::Plan { cmd } => match cmd {
            PlanCommands::Run {
                file,
                pattern,
                sa_mode: _,
                vars,
                tool,
                dry_run,
                cd,
            } => {
                plan_cmd::handle_plan_run(file, pattern, vars, tool, dry_run, cd, current_depth)
                    .await?;
            }
        },
        Commands::Migrate { dry_run, status } => {
            migrate_cmd::handle_migrate(dry_run, status)?;
        }
        Commands::SelfUpdate { check } => {
            self_update::handle_self_update(check)?;
        }
        Commands::ClaudeSubAgent(args) => {
            let exit_code =
                claude_sub_agent_cmd::handle_claude_sub_agent(args, current_depth).await?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(exit_code);
        }
        Commands::Tokuin { cmd } => {
            handle_tokuin(cmd)?;
        }
        Commands::Xurl { cmd } => {
            handle_xurl(cmd)?;
        }
    }

    Ok(())
}
