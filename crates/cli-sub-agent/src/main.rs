use anyhow::Result;
use clap::Parser;
mod arch_cmd;
mod audit;
mod audit_cmds;
mod batch;
mod bug_class;
mod build_jobs_env;
mod caller_hints_tests;
mod checklist_cmd;
mod claude_sub_agent_cmd;
mod cli;
mod codex_transcript_filter;
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
mod executor_csa_guard;
mod failover_trace;
mod gc;
mod gh_env;
mod goal_loop;
mod hooks_cmd;
mod hunt_cmd;
#[cfg(test)]
mod main_auto_weave_tests;
mod main_bootstrap;
mod mcp_hub;
mod mcp_server;
mod memory_capture;
mod memory_cmd;
mod memory_migrate;
mod merge_cmd;
mod mktsk_cmd;
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
mod plan_cmd_journal;
mod plan_condition;
mod plan_display;
mod preflight_state_dir;
mod preflight_symlink;
mod process_exit;
mod process_tree;
mod push_cmd;
mod recall_cmd;
mod review_cmd;
mod review_consensus;
mod review_context;
mod review_design_anchor;
mod review_findings;
mod review_gate;
mod review_prior_rounds;
mod review_routing;
mod review_session_findings;
mod run_cmd;
mod run_cmd_caller_fork;
mod run_cmd_daemon;
mod run_cmd_fork;
mod run_cmd_model_pin;
mod run_cmd_post;
mod run_cmd_preflight;
mod run_cmd_tool_selection;
mod run_helpers;
mod run_helpers_branch_guard;
#[cfg(test)]
mod sa_mode_tests;
mod self_update;
mod session_cmds;
mod session_cmds_daemon;
mod session_cmds_result;
mod session_cmds_result_measure;
mod session_dispatch;
mod session_guard;
mod session_observability;
mod session_outcome;
mod session_summary_text;
mod setup_cmds;
mod skill_cmds;
mod skill_dispatch;
mod skill_repo;
mod skill_resolver;
mod skill_run_cmd;
mod startup_env;
mod stdout_write;
#[cfg(any(feature = "parallel-tasks", test))]
pub mod task_lock;
#[cfg(test)]
mod test_env_lock;
#[cfg(test)]
mod test_session_sandbox;
mod tier_model_fallback;
mod tiers_cmd;
mod todo_cmd;
mod todo_epic_cmd;
mod todo_errors_cmd;
mod todo_ref_cmd;
mod tool_version;
mod triage_cmd;
mod verdict_exit_code;
mod verify_cmd;
mod xurl_cmd;
#[cfg(test)]
include!("review_cmd_exact_tests.rs");
#[cfg(test)]
include!("review_round10_exact_tests.rs");
#[cfg(test)]
include!("debate_cmd_exact_tests.rs");
use cli::{
    Cli, Commands, ConfigCommands, DoctorSubcommand, McpHubCommands, SetupCommands, TiersCommands,
    TodoCommands, TodoRefCommands, validate_command_args,
};
use csa_core::types::OutputFormat;
#[cfg(test)]
use main_bootstrap::should_attempt_auto_weave_upgrade;
use main_bootstrap::{
    link_bug_class_pipeline, maybe_auto_weave_upgrade, resolve_effective_min_timeout,
};
pub(crate) use process_exit::exit_current_process;
use process_exit::report_daemon_error_or_exit_code;
use sa_mode::apply_sa_mode_prompt_guard;

mod migrate_cmd;
mod sa_mode;

#[cfg(test)]
pub(crate) use sa_mode::validate_sa_mode;

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

    let mut startup_env = startup_env::StartupSubtreeEnv::capture_from_process_env();
    let current_depth = startup_env.current_depth();

    // Initialize tracing (output to stderr, initialize only once)
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let cli = Cli::parse_from(cli::normalize_epic_format_args(std::env::args_os()));
    let output_format = cli.format;
    let text_output = matches!(output_format, OutputFormat::Text);
    let command = cli.command;

    let min_timeout = resolve_effective_min_timeout();

    if let Err(err) = validate_command_args(&command, min_timeout) {
        err.exit();
    }
    executor_csa_guard::enforce(&command)?;

    let sa_mode_active = apply_sa_mode_prompt_guard(
        &command,
        current_depth,
        startup_env.internal_invocation(),
        output_format,
    )?;

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

    maybe_auto_weave_upgrade(&command).await;

    let legacy_xdg_paths = csa_config::paths::legacy_paths_requiring_migration();
    if !legacy_xdg_paths.is_empty() {
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
            fork_from_caller,
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
            allow_fallback,
            no_failover,
            fast_but_more_cost,
            build_jobs,
            wait,
            idle_timeout,
            initial_response_timeout,
            timeout,
            no_idle_timeout,
            no_memory,
            memory_query,
            stream_stdout,
            no_stream_stdout,
            no_error_marker_scan,
            no_preflight,
            no_post_exec_gate,
            require_commit,
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
            run_cmd_preflight::run_before_daemon_spawn_if_needed(
                cd.as_deref(),
                no_preflight,
                effective_no_daemon,
                daemon_child,
                session_id.is_some(),
                session.is_some() || last || fork_from.is_some() || fork_last,
            )?;
            let mut daemon_guard = run_cmd_daemon::check_daemon_flags(
                "run",
                effective_no_daemon,
                daemon_child,
                &session_id,
                cd.as_deref(),
                &mut startup_env,
                run_cmd_daemon::DaemonSpawnOptions::for_run(
                    skill.as_deref(),
                    prompt.as_deref(),
                    prompt_flag.as_deref(),
                    prompt_file.as_deref(),
                    no_fs_sandbox,
                    &extra_writable,
                ),
            )?;

            let stream_mode = if no_stream_stdout {
                csa_process::StreamMode::BufferOnly
            } else if stream_stdout || text_output {
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
                fork_from_caller,
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
                allow_fallback,
                no_failover,
                fast_but_more_cost,
                build_jobs,
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
                no_error_marker_scan,
                no_preflight,
                no_post_exec_gate,
                require_commit,
                extra_writable,
                extra_readable,
                startup_env: startup_env.clone(),
            })
            .await;
            let exit_code = report_daemon_error_or_exit_code(result, &mut daemon_guard);
            // Post-session SA mode reminder so caller sees constraint before next action.
            crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
                sa_mode_active,
                current_depth,
                text_output,
            );
            daemon_guard.finalize();
            exit_current_process(exit_code);
        }
        Commands::Hunt(args) => {
            let exit_code = hunt_cmd::handle_hunt(
                args.description,
                args.tool,
                args.timeout,
                args.allow_base_branch_working,
                current_depth,
                output_format,
                &startup_env,
            )
            .await?;
            exit_current_process(exit_code);
        }
        Commands::Arch(args) => {
            let exit_code =
                arch_cmd::handle_arch_args(args, current_depth, output_format, &startup_env)
                    .await?;
            exit_current_process(exit_code);
        }
        Commands::Triage(args) => {
            let exit_code = triage_cmd::handle_triage(
                args.description,
                args.tool,
                args.timeout,
                args.allow_base_branch_working,
                current_depth,
                output_format,
                &startup_env,
            )
            .await?;
            exit_current_process(exit_code);
        }
        Commands::Mktsk(args) => {
            let exit_code =
                mktsk_cmd::handle_mktsk_args(args, current_depth, output_format, &startup_env)
                    .await?;
            exit_current_process(exit_code);
        }
        Commands::Session { cmd } => session_dispatch::dispatch(cmd, output_format, &startup_env)?,
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
        Commands::Gc(args) => gc::handle_gc_args(args, output_format, startup_env.session_id())?,
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
                &mut startup_env,
                run_cmd_daemon::DaemonSpawnOptions::default(),
            )?;
            let result = review_cmd::handle_review(args, current_depth, &startup_env).await;
            let exit_code = report_daemon_error_or_exit_code(result, &mut daemon_guard);
            crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
                sa_mode_active,
                current_depth,
                text_output,
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
                &mut startup_env,
                run_cmd_daemon::DaemonSpawnOptions::for_prompt_file(args.prompt_file.as_deref()),
            )?;
            let result =
                debate_cmd::handle_debate(args, current_depth, output_format, &startup_env).await;
            let exit_code = report_daemon_error_or_exit_code(result, &mut daemon_guard);
            crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
                sa_mode_active,
                current_depth,
                text_output,
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
            batch::handle_batch(file, cd, dry_run, current_depth, &startup_env).await?;
            crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
                sa_mode_active,
                current_depth,
                text_output,
            );
        }
        Commands::McpServer => {
            mcp_server::run_mcp_server(&startup_env).await?;
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
        Commands::Skill { cmd } => {
            let code =
                skill_dispatch::dispatch(cmd, current_depth, output_format, &startup_env).await?;
            if code != 0 {
                exit_current_process(code);
            }
        }
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
            SetupCommands::ReviewGate { check } => {
                let project_root = std::env::current_dir()?;
                setup_cmds::handle_setup_review_gate(&project_root, check)?;
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
            TodoCommands::Attest { timestamp, cd } => todo_cmd::handle_attest(timestamp, cd)?,
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
            TodoCommands::Errors { branch, cd } => {
                todo_errors_cmd::handle_errors(branch, cd)?;
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
                text_output,
                &startup_env,
            )
            .await?;
        }
        Commands::Migrate { dry_run, status } => migrate_cmd::handle_migrate(dry_run, status)?,
        Commands::SelfUpdate { check } => self_update::handle_self_update(check)?,
        Commands::ClaudeSubAgent(args) => {
            let exit_code =
                claude_sub_agent_cmd::handle_claude_sub_agent(args, current_depth, &startup_env)
                    .await?;
            crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
                sa_mode_active,
                current_depth,
                text_output,
            );
            exit_current_process(exit_code);
        }
        Commands::Tokuin { cmd } => cli::handle_tokuin(cmd)?,
        Commands::Verify(args) => {
            let exit_code = verify_cmd::handle_verify(args)?;
            exit_current_process(exit_code);
        }
        Commands::Health(args) => cli::handle_health(args)?,
        Commands::Xurl { cmd } => xurl_cmd::handle_xurl(cmd)?,
        Commands::Recall(args) => recall_cmd::handle_recall(args.cmd)?,
        Commands::Hooks { cmd } => hooks_cmd::handle_hooks(cmd)?,
    }

    Ok(())
}
