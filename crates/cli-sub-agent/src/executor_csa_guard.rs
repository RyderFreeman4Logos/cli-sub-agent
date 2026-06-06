//! Runtime guard for `csa` commands invoked from skill executor sessions.

use std::collections::HashMap;

use anyhow::Result;

use crate::cli::{Commands, SkillCommands};

const CSA_SKILL_MODE_ENV: &str = "CSA_SKILL_MODE";
const CSA_SKILL_MODE_EXECUTOR: &str = "executor";

pub(crate) fn enforce(command: &Commands) -> Result<()> {
    if !is_active() {
        return Ok(());
    }

    enforce_active(command)
}

fn enforce_active(command: &Commands) -> Result<()> {
    if let Some(skill) = recursive_dev2merge_skill(command) {
        anyhow::bail!(
            "executor mode blocks recursive dev2merge invocation `csa run --skill {skill}`"
        );
    }

    tracing::info!("allowed non-recursive csa command in executor mode");
    Ok(())
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

fn recursive_dev2merge_skill(command: &Commands) -> Option<&str> {
    match command {
        Commands::Run {
            skill: Some(skill), ..
        } if is_dev2merge_skill(skill) => Some(skill.as_str()),
        Commands::Skill {
            cmd: SkillCommands::Run { name, .. },
        } if is_dev2merge_skill(name) => Some(name.as_str()),
        _ => None,
    }
}

fn is_dev2merge_skill(skill: &str) -> bool {
    matches!(skill, "dev2merge" | "dev-to-merge")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    fn parse_command(args: &[&str]) -> Commands {
        Cli::try_parse_from(args)
            .expect("test command should parse")
            .command
    }

    fn assert_executor_guard_allows(args: &[&str]) {
        let command = parse_command(args);
        enforce_active(&command).expect("executor guard should allow command");
    }

    fn assert_executor_guard_blocks(args: &[&str]) {
        let command = parse_command(args);
        let err = enforce_active(&command).expect_err("executor guard should block command");
        assert!(
            err.to_string().contains("recursive dev2merge invocation"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn executor_guard_allows_review() {
        assert_executor_guard_allows(&["csa", "review", "--diff"]);
    }

    #[test]
    fn executor_guard_allows_debate() {
        assert_executor_guard_allows(&["csa", "debate", "question"]);
    }

    #[test]
    fn executor_guard_blocks_dev2merge_skill_run() {
        assert_executor_guard_blocks(&["csa", "run", "--skill", "dev2merge", "prompt"]);
    }

    #[test]
    fn executor_guard_blocks_dev_to_merge_skill_run() {
        assert_executor_guard_blocks(&["csa", "run", "--skill", "dev-to-merge", "prompt"]);
    }

    #[test]
    fn executor_guard_blocks_dev2merge_skill_subcommand_run() {
        assert_executor_guard_blocks(&["csa", "skill", "run", "dev2merge", "prompt"]);
    }

    #[test]
    fn executor_guard_blocks_dev_to_merge_skill_subcommand_run() {
        assert_executor_guard_blocks(&["csa", "skill", "run", "dev-to-merge", "prompt"]);
    }

    #[test]
    fn executor_guard_allows_mktd_skill_run() {
        assert_executor_guard_allows(&["csa", "run", "--skill", "mktd", "prompt"]);
    }

    #[test]
    fn executor_guard_allows_general_run() {
        assert_executor_guard_allows(&["csa", "run", "prompt"]);
    }
}
