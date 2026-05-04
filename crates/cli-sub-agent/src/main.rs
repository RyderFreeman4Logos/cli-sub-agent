use std::io::Write;

use anyhow::Result;
use clap::Parser;

mod audit;
mod audit_cmds;
mod batch;
mod bug_class;
mod caller_hints_tests;
mod checklist_cmd;
mod claude_sub_agent_cmd;
mod cli;
mod config_cmds;
mod debate_cmd;
mod debate_cmd_output;
mod debate_cmd_resolve;
mod debate_errors;
mod difficulty_routing;
mod doctor;
mod edit_restriction_guard;
mod error_hints;
mod error_report;
mod eval_cmd;
mod gc;
mod goal_loop;
mod hooks_cmd;
mod hunt_cmd;
mod main_bootstrap;
mod mcp_hub;
mod mcp_server;
mod memory_capture;
mod memory_cmd;
mod memory_migrate;
mod merge_cmd;
mod pattern_resolver;
mod pipeline;
mod pipeline_env;
mod pipeline_execute;
mod pipeline_handoff;
mod pipeline_jj_journal;
mod pipeline_post_exec;
mod pipeline_project_key;
mod pipeline_sandbox;
mod pipeline_transcript;
mod plan_cmd;
mod plan_cmd_daemon;
mod plan_condition;
mod plan_display;
mod preflight_state_dir;
mod preflight_symlink;
mod process_tree;
mod push_cmd;
mod review_cmd;
mod review_consensus;
mod review_context;
mod review_design_anchor;
mod review_findings;
mod review_prior_rounds;
mod review_routing;
mod review_session_findings;
mod run_cmd;
mod run_cmd_daemon;
mod run_cmd_fork;
mod run_cmd_post;
mod run_cmd_tool_selection;
mod run_helpers;
mod run_helpers_branch_guard;
mod self_update;
mod session_cmds;
mod session_cmds_daemon;
mod session_cmds_result;
mod session_dispatch;
mod session_guard;
mod session_observability;
mod setup_cmds;
mod skill_cmds;
mod skill_resolver;
mod tier_model_fallback;
mod tiers_cmd;
mod todo_cmd;
mod todo_epic_cmd;
mod todo_ref_cmd;
mod tool_version;

#[cfg(test)]
mod sa_mode_tests;

#[cfg(test)]
mod main_auto_weave_tests;

#[cfg(test)]
mod test_env_lock;
#[cfg(test)]
mod test_session_sandbox;
#[cfg(test)]
include!("review_cmd_exact_tests.rs");
#[cfg(test)]
include!("review_round10_exact_tests.rs");
#[cfg(test)]
include!("debate_cmd_exact_tests.rs");

use cli::{
    Cli, Commands, ConfigCommands, DoctorSubcommand, McpHubCommands, SetupCommands, SkillCommands,
    TiersCommands, TodoCommands, TodoRefCommands, handle_tokuin, handle_xurl,
    validate_command_args,
};
use csa_core::types::OutputFormat;
use main_bootstrap::{
    link_bug_class_pipeline, resolve_effective_min_timeout, should_attempt_auto_weave_upgrade,
};
use sa_mode::apply_sa_mode_prompt_guard;

mod migrate_cmd;
mod sa_mode;

// Re-export for tests that reference `crate::validate_sa_mode`.
#[cfg(test)]
pub(crate) use sa_mode::validate_sa_mode;

/// Report a daemon command error to stderr while the daemon guard still holds
/// fd 2 open, then return an exit code.
///
/// Without this, `?`-propagated errors would drop the guard (closing fd 2)
/// *before* `main()` could print the error via `eprintln!`, causing EBADF →
/// double-panic → process abort (issue #574).
fn report_daemon_error_or_exit_code(
    result: Result<i32>,
    daemon_guard: &mut run_cmd_daemon::DaemonChildGuard,
) -> i32 {
    match result {
        Ok(code) => code,
        Err(err) => {
            // fd 2 is still the pipe write-end (guard is alive), so eprintln! works.
            eprintln!("{}", error_report::render_user_facing_error(&err));
            if let Some(hint) = error_hints::suggest_fix(&err) {
                eprintln!();
                eprintln!("{hint}");
            }
            // Finalize the guard explicitly so stderr.log captures the error above.
            daemon_guard.finalize();
            exit_current_process(1);
        }
    }
}

fn exit_current_process(exit_code: i32) -> ! {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    crate::session_cmds_daemon::persist_daemon_completion_from_env(exit_code);
    std::process::exit(exit_code);
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("{}", error_report::render_user_facing_error(&err));
        if let Some(hint) = error_hints::suggest_fix(&err) {
            eprintln!();
            eprintln!("{hint}");
        }
        exit_current_process(1);
    }
}

async fn run() -> Result<()> {
    link_bug_class_pipeline();

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

    let cli = Cli::parse_from(cli::normalize_epic_format_args(std::env::args_os()));
    let output_format = cli.format;
    let command = cli.command;

    // Resolve effective min_timeout_seconds from configs (project overrides global).
    // This is a lightweight load; config errors are ignored (fall back to compile-time default).
    let min_timeout = resolve_effective_min_timeout();

    if let Err(err) = validate_command_args(&command, min_timeout) {
        err.exit();
    }

    let sa_mode_active = apply_sa_mode_prompt_guard(&command, current_depth, output_format)?;

    // Check weave.lock version alignment (non-fatal, read-only).
    if let Ok(cwd) = std::env::current_dir() {
        let registry = csa_config::default_registry();
        match csa_config::check_version(
            &cwd,
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_VERSION"),
            &registry,
        ) {
            Ok(result) => {
                if let Some(warning) = csa_config::weave_lock::format_version_check_warning(&result)
                {
                    eprintln!("{warning}");
                }
            }
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
            && should_attempt_auto_weave_upgrade(&command)
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

                let ok = result.as_ref().map(|s| s.success()).unwrap_or(false);
                if ok {
                    success = true;
                    break;
                }
                if attempt < 2 {
                    tracing::debug!(
                        "weave upgrade attempt {attempt} failed, retrying in {delay:?}"
                    );
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
            }

            if !success {
                let msg = "auto weave upgrade failed after 3 attempts (non-fatal). \
                           Disable with [execution] auto_weave_upgrade = false";
                tracing::warn!(msg);
                eprintln!("warning: {msg}");
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
            auto_route,
            hint_difficulty,
            skill,
            sa_mode: _,
            prompt,
            goal,
            prompt_flag,
            prompt_file,
            inline_context_from_review_session,
            session,
            last,
            fork_from,
            fork_last,
            description,
            fork_call,
            return_to,
            parent,
            ephemeral,
            allow_base_branch_working,
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
            extra_readable,
            daemon: _daemon,
            no_daemon,
            daemon_child,
            session_id,
        } => {
            if !no_daemon
                && !daemon_child
                && session_id.is_none()
                && let Some(exit_code) = run_helpers_branch_guard::evaluate_run_refusal_for_cd(
                    allow_base_branch_working,
                    cd.as_deref(),
                )?
            {
                exit_current_process(exit_code);
            }
            let effective_no_daemon = no_daemon || goal.is_some();
            let mut daemon_guard = run_cmd_daemon::check_daemon_flags(
                "run",
                effective_no_daemon,
                daemon_child,
                &session_id,
                cd.as_deref(),
                run_cmd_daemon::DaemonSpawnOptions::for_run(
                    skill.as_deref(),
                    prompt.as_deref(),
                    prompt_flag.as_deref(),
                    prompt_file.as_deref(),
                    no_fs_sandbox,
                    &extra_writable,
                ),
            )?;

            // Daemon child path: continue with normal run logic and resolve stream mode.
            let stream_mode = if no_stream_stdout {
                csa_process::StreamMode::BufferOnly
            } else if stream_stdout || matches!(output_format, OutputFormat::Text) {
                csa_process::StreamMode::TeeToStderr
            } else {
                csa_process::StreamMode::BufferOnly
            };

            let result = goal_loop::handle_run_or_goal(goal_loop::GoalRunRequest {
                goal_criteria: goal,
                tool,
                auto_route,
                hint_difficulty,
                skill,
                prompt,
                prompt_flag,
                prompt_file,
                inline_context_from_review_session,
                session,
                last,
                fork_from,
                fork_last,
                description,
                fork_call,
                return_to,
                parent,
                ephemeral,
                allow_base_branch_working,
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
                extra_readable,
            })
            .await;
            // Report errors while fd 2 is still open (guard holds stderr rotation).
            // Dropping the guard closes fd 2, so eprintln! must happen first.
            let exit_code = report_daemon_error_or_exit_code(result, &mut daemon_guard);
            // Post-session SA mode reminder so caller sees constraint before next action.
            crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
                sa_mode_active,
                current_depth,
                matches!(output_format, OutputFormat::Text),
            );
            daemon_guard.finalize();
            exit_current_process(exit_code);
        }
        Commands::Hunt {
            description,
            tool,
            timeout,
            allow_base_branch_working,
        } => {
            let exit_code = hunt_cmd::handle_hunt(
                description,
                tool,
                timeout,
                allow_base_branch_working,
                current_depth,
                output_format,
            )
            .await?;
            exit_current_process(exit_code);
        }
        Commands::Session { cmd } => session_dispatch::dispatch(cmd, output_format)?,
        Commands::Push(args) => push_cmd::handle_push(args)?,
        Commands::Merge(args) => merge_cmd::handle_merge(args)?,
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
        Commands::Gc(gc::GcArgs {
            dry_run,
            max_age_days,
            reap_runtime,
            global,
        }) => {
            if global {
                gc::handle_gc_global(dry_run, max_age_days, reap_runtime, output_format)?;
            } else {
                gc::handle_gc(dry_run, max_age_days, reap_runtime, output_format)?;
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
            ConfigCommands::Set {
                key,
                value,
                project,
                cd,
                ..
            } => {
                config_cmds::handle_config_set(key, value, project, cd)?;
            }
        },
        Commands::Memory { command } => {
            memory_cmd::handle_memory_command(command).await?;
        }
        Commands::Review(args) => {
            let mut daemon_guard = run_cmd_daemon::check_daemon_flags(
                "review",
                args.no_daemon || args.check_verdict,
                args.daemon_child,
                &args.session_id,
                args.cd.as_deref(),
                run_cmd_daemon::DaemonSpawnOptions::default(),
            )?;
            let result = review_cmd::handle_review(args, current_depth).await;
            let exit_code = report_daemon_error_or_exit_code(result, &mut daemon_guard);
            crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
                sa_mode_active,
                current_depth,
                matches!(output_format, OutputFormat::Text),
            );
            daemon_guard.finalize();
            exit_current_process(exit_code);
        }
        Commands::Debate(args) => {
            let mut daemon_guard = run_cmd_daemon::check_daemon_flags(
                "debate",
                args.no_daemon || args.dry_run,
                args.daemon_child,
                &args.session_id,
                args.cd.as_deref(),
                run_cmd_daemon::DaemonSpawnOptions::for_prompt_file(args.prompt_file.as_deref()),
            )?;
            let result = debate_cmd::handle_debate(args, current_depth, output_format).await;
            let exit_code = report_daemon_error_or_exit_code(result, &mut daemon_guard);
            crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
                sa_mode_active,
                current_depth,
                matches!(output_format, OutputFormat::Text),
            );
            daemon_guard.finalize();
            exit_current_process(exit_code);
        }
        Commands::Eval {
            passive: _,
            project,
            days,
            json,
        } => {
            eval_cmd::handle_eval(project, days, json)?;
        }
        Commands::Doctor { subcommand } => match subcommand {
            None => doctor::run_doctor(output_format).await?,
            Some(DoctorSubcommand::Routing { operation, tier }) => {
                doctor::run_doctor_routing(output_format, operation, tier).await?
            }
        },
        Commands::Batch {
            file,
            sa_mode: _,
            cd,
            dry_run,
        } => {
            batch::handle_batch(file, cd, dry_run, current_depth).await?;
            crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
                sa_mode_active,
                current_depth,
                matches!(output_format, OutputFormat::Text),
            );
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
            TodoCommands::Update {
                timestamp,
                title,
                status,
                description,
                cd,
            } => {
                todo_cmd::handle_update(timestamp, title, status, description, cd)?;
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
            TodoCommands::Epic { command } => {
                todo_epic_cmd::handle_epic_command(command)?;
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
        Commands::Checklist { command } => checklist_cmd::handle_checklist_command(command)?,
        Commands::Plan { cmd } => {
            plan_cmd_daemon::dispatch(
                cmd,
                current_depth,
                sa_mode_active,
                matches!(output_format, OutputFormat::Text),
            )
            .await?;
        }
        Commands::Migrate { dry_run, status } => {
            migrate_cmd::handle_migrate(dry_run, status)?;
        }
        Commands::SelfUpdate { check } => {
            self_update::handle_self_update(check)?;
        }
        Commands::ClaudeSubAgent(args) => {
            let exit_code =
                claude_sub_agent_cmd::handle_claude_sub_agent(args, current_depth).await?;
            crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
                sa_mode_active,
                current_depth,
                matches!(output_format, OutputFormat::Text),
            );
            exit_current_process(exit_code);
        }
        Commands::Tokuin { cmd } => {
            handle_tokuin(cmd)?;
        }
        Commands::Xurl { cmd } => {
            handle_xurl(cmd)?;
        }
        Commands::Hooks { cmd } => {
            hooks_cmd::handle_hooks(cmd)?;
        }
    }

    Ok(())
}
