//! Dispatch handler for all `csa skill` subcommands.

use anyhow::Result;
use csa_core::types::OutputFormat;

use crate::cli::SkillCommands;
use crate::{skill_cmds, skill_run_cmd};

/// Dispatch a `csa skill` subcommand, returning an exit code.
pub(crate) async fn dispatch(
    cmd: SkillCommands,
    current_depth: u32,
    output_format: OutputFormat,
) -> Result<i32> {
    match cmd {
        SkillCommands::Install { source, target } => {
            skill_cmds::handle_skill_install(source, target)?;
        }
        SkillCommands::List => {
            skill_cmds::handle_skill_list()?;
        }
        SkillCommands::Add { name } => {
            skill_cmds::handle_skill_add(name)?;
        }
        SkillCommands::Edit { name } => {
            skill_cmds::handle_skill_edit(name)?;
        }
        SkillCommands::Scan => {
            skill_cmds::handle_skill_scan()?;
        }
        SkillCommands::Backup => {
            skill_cmds::handle_skill_backup()?;
        }
        SkillCommands::Run { name, prompt } => {
            return skill_run_cmd::handle_skill_run(name, prompt, current_depth, output_format)
                .await;
        }
    }
    Ok(0)
}
