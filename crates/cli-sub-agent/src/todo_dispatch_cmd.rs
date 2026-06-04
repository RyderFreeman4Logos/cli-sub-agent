use anyhow::Result;
use csa_core::types::OutputFormat;

use crate::cli::{TodoCommands, TodoRefCommands};

pub(crate) fn handle_todo_command(cmd: TodoCommands, output_format: OutputFormat) -> Result<()> {
    match cmd {
        TodoCommands::Create {
            title,
            branch,
            no_branch,
            language,
            cd,
        } => {
            crate::todo_cmd::handle_create(title, branch, no_branch, language, cd, output_format)?;
        }
        TodoCommands::Save {
            timestamp,
            message,
            cd,
        } => {
            crate::todo_cmd::handle_save(timestamp, message, cd)?;
        }
        cmd @ TodoCommands::Persist { .. } => crate::todo_persist_cmd::handle_command(cmd)?,
        TodoCommands::Attest { timestamp, cd } => crate::todo_cmd::handle_attest(timestamp, cd)?,
        TodoCommands::Diff {
            timestamp,
            revision,
            from,
            to,
            cd,
        } => {
            crate::todo_cmd::handle_diff(timestamp, revision, from, to, cd)?;
        }
        TodoCommands::History { timestamp, cd } => {
            crate::todo_cmd::handle_history(timestamp, cd)?;
        }
        TodoCommands::List { status, cd } => {
            crate::todo_cmd::handle_list(status, cd, output_format)?;
        }
        TodoCommands::Find { branch, status, cd } => {
            crate::todo_cmd::handle_find(branch, status, cd, output_format)?;
        }
        TodoCommands::Errors { branch, cd } => {
            crate::todo_errors_cmd::handle_errors(branch, cd)?;
        }
        TodoCommands::Show {
            timestamp,
            version,
            path,
            spec,
            refs,
            cd,
        } => {
            crate::todo_cmd::handle_show(timestamp, version, path, spec, refs, cd)?;
        }
        TodoCommands::Update {
            timestamp,
            title,
            status,
            description,
            cd,
        } => {
            crate::todo_cmd::handle_update(timestamp, title, status, description, cd)?;
        }
        TodoCommands::Status {
            timestamp,
            status,
            cd,
        } => {
            crate::todo_cmd::handle_status(timestamp, status, cd)?;
        }
        TodoCommands::Dag {
            timestamp,
            format,
            cd,
        } => {
            crate::todo_cmd::handle_dag(timestamp, format, cd)?;
        }
        TodoCommands::Epic { command } => {
            crate::todo_epic_cmd::handle_epic_command(command)?;
        }
        TodoCommands::Ref { cmd } => match cmd {
            TodoRefCommands::List {
                timestamp,
                tokens,
                json,
                cd,
            } => {
                crate::todo_cmd::handle_ref_list(timestamp, tokens, json, cd)?;
            }
            TodoRefCommands::Show {
                timestamp,
                name,
                max_tokens,
                cd,
            } => {
                crate::todo_cmd::handle_ref_show(timestamp, name, max_tokens, cd)?;
            }
            TodoRefCommands::Add {
                timestamp,
                name,
                content,
                file,
                cd,
            } => {
                crate::todo_cmd::handle_ref_add(timestamp, name, content, file, cd)?;
            }
            TodoRefCommands::ImportTranscript {
                timestamp,
                tool,
                session,
                name,
                cd,
            } => {
                crate::todo_cmd::handle_ref_import_transcript(timestamp, tool, session, name, cd)?;
            }
        },
    }

    Ok(())
}
