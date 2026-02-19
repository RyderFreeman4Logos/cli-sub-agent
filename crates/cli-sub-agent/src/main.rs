use std::io::IsTerminal;

use anyhow::Result;
use clap::Parser;

mod audit;
mod audit_cmds;
mod batch;
mod claude_sub_agent_cmd;
mod cli;
mod config_cmds;
mod debate_cmd;
mod doctor;
mod gc;
mod mcp_server;
mod pattern_resolver;
mod pipeline;
mod pipeline_env;
mod pipeline_sandbox;
mod plan_cmd;
mod process_tree;
mod review_cmd;
mod review_consensus;
mod run_cmd;
mod run_helpers;
mod self_update;
mod session_cmds;
mod session_guard;
mod setup_cmds;
mod skill_cmds;
mod skill_resolver;
mod tiers_cmd;
mod todo_cmd;

use cli::{
    Cli, Commands, ConfigCommands, PlanCommands, SessionCommands, SetupCommands, SkillCommands,
    TiersCommands, TodoCommands,
};
use csa_core::types::OutputFormat;

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
            skill,
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
            force,
            no_failover,
            wait,
            idle_timeout,
            stream_stdout,
            no_stream_stdout,
        } => {
            // --stream-stdout forces streaming; --no-stream-stdout forces buffering;
            // default: stream when stderr is TTY and output format is Text.
            let stream_mode = if no_stream_stdout {
                csa_process::StreamMode::BufferOnly
            } else if stream_stdout
                || (matches!(output_format, OutputFormat::Text) && std::io::stderr().is_terminal())
            {
                csa_process::StreamMode::TeeToStderr
            } else {
                csa_process::StreamMode::BufferOnly
            };

            let exit_code = run_cmd::handle_run(
                tool,
                skill,
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
                force,
                no_failover,
                wait,
                idle_timeout,
                current_depth,
                output_format,
                stream_mode,
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
            SessionCommands::Result { session, json, cd } => {
                session_cmds::handle_session_result(session, json, cd)?;
            }
            SessionCommands::Artifacts { session, cd } => {
                session_cmds::handle_session_artifacts(session, cd)?;
            }
            SessionCommands::Log { session, cd } => {
                session_cmds::handle_session_log(session, cd)?;
            }
            SessionCommands::Checkpoint { session, cd } => {
                session_cmds::handle_session_checkpoint(session, cd)?;
            }
            SessionCommands::Checkpoints { cd } => {
                session_cmds::handle_session_checkpoints(cd)?;
            }
        },
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
        Commands::Review(args) => {
            let exit_code = review_cmd::handle_review(args, current_depth).await?;
            std::process::exit(exit_code);
        }
        Commands::Debate(args) => {
            let exit_code = debate_cmd::handle_debate(args, current_depth).await?;
            std::process::exit(exit_code);
        }
        Commands::Doctor => {
            doctor::run_doctor(output_format).await?;
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
        Commands::Todo { cmd } => match cmd {
            TodoCommands::Create { title, branch, cd } => {
                todo_cmd::handle_create(title, branch, cd, output_format)?;
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
                cd,
            } => {
                todo_cmd::handle_show(timestamp, version, path, cd)?;
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
        },
        Commands::Plan { cmd } => match cmd {
            PlanCommands::Run {
                file,
                vars,
                dry_run,
                cd,
            } => {
                plan_cmd::handle_plan_run(file, vars, dry_run, cd, current_depth).await?;
            }
        },
        Commands::SelfUpdate { check } => {
            self_update::handle_self_update(check)?;
        }
        Commands::ClaudeSubAgent(args) => {
            let exit_code =
                claude_sub_agent_cmd::handle_claude_sub_agent(args, current_depth).await?;
            std::process::exit(exit_code);
        }
    }

    Ok(())
}
