//! Session subcommand dispatch — extracted from main.rs to stay under
//! the monolith file limit.

use std::io::Write;

use anyhow::Result;

use crate::cli::SessionCommands;
use crate::session_cmds;
use csa_core::types::OutputFormat;

pub(crate) fn dispatch(cmd: SessionCommands, output_format: OutputFormat) -> Result<()> {
    match cmd {
        SessionCommands::List {
            cd,
            branch,
            tool,
            tree,
        } => {
            session_cmds::handle_session_list(cd, branch, tool, tree, output_format)?;
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
        SessionCommands::Logs {
            session,
            tail,
            events,
            cd,
        } => {
            session_cmds::handle_session_logs(session, tail, events, cd)?;
        }
        SessionCommands::IsAlive { session, cd } => {
            let alive = session_cmds::handle_session_is_alive(session, cd)?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(if alive { 0 } else { 1 });
        }
        SessionCommands::Result {
            session,
            json,
            summary,
            section,
            full,
            cd,
        } => {
            session_cmds::handle_session_result(
                session,
                json,
                cd,
                session_cmds::StructuredOutputOpts {
                    summary,
                    section,
                    full,
                },
            )?;
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
        SessionCommands::Measure { session, json, cd } => {
            session_cmds::handle_session_measure(session, json, cd)?;
        }
        SessionCommands::ToolOutput {
            session,
            index,
            list,
            cd,
        } => {
            session_cmds::handle_session_tool_output(session, index, list, cd)?;
        }
        SessionCommands::Wait {
            session,
            timeout,
            cd,
        } => {
            let exit_code = session_cmds::handle_session_wait(session, timeout, cd)?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(exit_code);
        }
        SessionCommands::Attach {
            session,
            stderr,
            cd,
        } => {
            let exit_code = session_cmds::handle_session_attach(session, stderr, cd)?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(exit_code);
        }
    }
    Ok(())
}
