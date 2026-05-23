//! Runtime guard for `csa` commands invoked from skill executor sessions.

use std::collections::HashMap;

use anyhow::Result;

use crate::cli::{
    Commands, ConfigCommands, PlanCommands, TodoCommands, TodoRefCommands, TokuinCommands,
};

const CSA_SKILL_MODE_ENV: &str = "CSA_SKILL_MODE";
const CSA_SKILL_MODE_EXECUTOR: &str = "executor";

const EXECUTOR_SAFE_CSA_COMMAND_PREFIXES: &[&[&str]] = &[
    &["todo", "create"],
    &["todo", "save"],
    &["todo", "attest"],
    &["todo", "ref", "add"],
    &["todo", "ref", "list"],
    &["todo", "ref", "show"],
    &["todo", "show"],
    &["todo", "list"],
    &["todo", "find"],
    &["tokuin", "estimate"],
    &["tokuin", "models"],
    &["config", "get"],
    &["config", "show"],
];

pub(crate) fn enforce(command: &Commands) -> Result<()> {
    if !is_active() {
        return Ok(());
    }

    let prefix = command_prefix(command);
    if is_safe_prefix(&prefix) {
        tracing::info!(
            command = %format_command_prefix(&prefix),
            "allowed non-recursive csa command in executor mode"
        );
        return Ok(());
    }

    anyhow::bail!(
        "executor mode blocks recursive csa command `{}`. Allowed non-recursive csa command prefixes: {}",
        format_command_prefix(&prefix),
        format_safe_prefixes(),
    );
}

pub(crate) fn mark_skill_executor_env(env: &mut Option<HashMap<String, String>>, is_skill: bool) {
    if is_skill {
        env.get_or_insert_with(Default::default).insert(
            CSA_SKILL_MODE_ENV.to_string(),
            CSA_SKILL_MODE_EXECUTOR.to_string(),
        );
    }
}

fn is_active() -> bool {
    std::env::var(CSA_SKILL_MODE_ENV)
        .map(|value| value.trim().eq_ignore_ascii_case(CSA_SKILL_MODE_EXECUTOR))
        .unwrap_or(false)
}

fn is_safe_prefix(prefix: &[&str]) -> bool {
    EXECUTOR_SAFE_CSA_COMMAND_PREFIXES.contains(&prefix)
}

fn command_prefix(command: &Commands) -> Vec<&'static str> {
    match command {
        Commands::Run { .. } => vec!["run"],
        Commands::Review(_) => vec!["review"],
        Commands::Debate(_) => vec!["debate"],
        Commands::Plan {
            cmd: PlanCommands::Run { .. },
        } => vec!["plan", "run"],
        Commands::Todo { cmd } => todo_prefix(cmd),
        Commands::Config { cmd } => config_prefix(cmd),
        Commands::Tokuin {
            cmd: TokuinCommands::Estimate { .. },
        } => vec!["tokuin", "estimate"],
        Commands::Tokuin {
            cmd: TokuinCommands::Models,
        } => vec!["tokuin", "models"],
        Commands::Hunt(_) => vec!["hunt"],
        Commands::Arch(_) => vec!["arch"],
        Commands::Triage(_) => vec!["triage"],
        Commands::Mktsk(_) => vec!["mktsk"],
        Commands::Session { .. } => vec!["session"],
        Commands::Push(_) => vec!["push"],
        Commands::Merge(_) => vec!["merge"],
        Commands::Audit { .. } => vec!["audit"],
        Commands::Init { .. } => vec!["init"],
        Commands::Gc(_) => vec!["gc"],
        Commands::Memory { .. } => vec!["memory"],
        Commands::Eval { .. } => vec!["eval"],
        Commands::Doctor { .. } => vec!["doctor"],
        Commands::Batch { .. } => vec!["batch"],
        Commands::McpServer => vec!["mcp-server"],
        Commands::McpHub { .. } => vec!["mcp-hub"],
        Commands::Skill { .. } => vec!["skill"],
        Commands::Setup { .. } => vec!["setup"],
        Commands::Tiers { .. } => vec!["tiers"],
        Commands::Checklist { .. } => vec!["checklist"],
        Commands::Dev2merge(_) => vec!["dev2merge"],
        Commands::Migrate { .. } => vec!["migrate"],
        Commands::SelfUpdate { .. } => vec!["self-update"],
        Commands::ClaudeSubAgent(_) => vec!["claude-sub-agent"],
        Commands::Xurl { .. } => vec!["xurl"],
        Commands::Recall(_) => vec!["recall"],
        Commands::Hooks { .. } => vec!["hooks"],
    }
}

fn todo_prefix(cmd: &TodoCommands) -> Vec<&'static str> {
    match cmd {
        TodoCommands::Create { .. } => vec!["todo", "create"],
        TodoCommands::Save { .. } => vec!["todo", "save"],
        TodoCommands::Attest { .. } => vec!["todo", "attest"],
        TodoCommands::List { .. } => vec!["todo", "list"],
        TodoCommands::Find { .. } => vec!["todo", "find"],
        TodoCommands::Errors { .. } => vec!["todo", "errors"],
        TodoCommands::Show { .. } => vec!["todo", "show"],
        TodoCommands::Ref { cmd } => match cmd {
            TodoRefCommands::Add { .. } => vec!["todo", "ref", "add"],
            TodoRefCommands::List { .. } => vec!["todo", "ref", "list"],
            TodoRefCommands::Show { .. } => vec!["todo", "ref", "show"],
            TodoRefCommands::ImportTranscript { .. } => vec!["todo", "ref", "import-transcript"],
        },
        TodoCommands::Diff { .. } => vec!["todo", "diff"],
        TodoCommands::History { .. } => vec!["todo", "history"],
        TodoCommands::Update { .. } => vec!["todo", "update"],
        TodoCommands::Status { .. } => vec!["todo", "status"],
        TodoCommands::Dag { .. } => vec!["todo", "dag"],
        TodoCommands::Epic { .. } => vec!["todo", "epic"],
    }
}

fn config_prefix(cmd: &ConfigCommands) -> Vec<&'static str> {
    match cmd {
        ConfigCommands::Get { .. } => vec!["config", "get"],
        ConfigCommands::Show { .. } => vec!["config", "show"],
        ConfigCommands::Edit { .. } => vec!["config", "edit"],
        ConfigCommands::Validate { .. } => vec!["config", "validate"],
        ConfigCommands::Set { .. } => vec!["config", "set"],
    }
}

fn format_command_prefix(prefix: &[&str]) -> String {
    format!("csa {}", prefix.join(" "))
}

fn format_safe_prefixes() -> String {
    EXECUTOR_SAFE_CSA_COMMAND_PREFIXES
        .iter()
        .map(|prefix| format_command_prefix(prefix))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_prefixes_allow_todo_persistence_commands() {
        for prefix in [
            &["todo", "create"][..],
            &["todo", "save"][..],
            &["todo", "ref", "add"][..],
            &["todo", "ref", "list"][..],
            &["todo", "ref", "show"][..],
            &["todo", "show"][..],
            &["todo", "list"][..],
            &["todo", "find"][..],
            &["tokuin", "estimate"][..],
            &["config", "get"][..],
            &["config", "show"][..],
        ] {
            assert!(
                is_safe_prefix(prefix),
                "prefix should be allowed: {prefix:?}"
            );
        }
    }

    #[test]
    fn safe_prefixes_block_recursive_execution_commands() {
        for prefix in [
            &["run"][..],
            &["review"][..],
            &["debate"][..],
            &["plan", "run"][..],
            &["todo", "ref", "import-transcript"][..],
            &["config", "set"][..],
        ] {
            assert!(
                !is_safe_prefix(prefix),
                "prefix should be blocked: {prefix:?}"
            );
        }
    }
}
