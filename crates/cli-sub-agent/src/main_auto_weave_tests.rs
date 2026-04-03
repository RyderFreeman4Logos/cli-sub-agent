use super::{Cli, should_attempt_auto_weave_upgrade};
use clap::Parser;

#[test]
fn todo_commands_skip_auto_weave_upgrade() {
    let cli = Cli::parse_from(["csa", "todo", "list"]);
    assert!(!should_attempt_auto_weave_upgrade(&cli.command));
}

#[test]
fn run_commands_still_attempt_auto_weave_upgrade() {
    let cli = Cli::parse_from(["csa", "run", "--sa-mode", "false", "status"]);
    assert!(should_attempt_auto_weave_upgrade(&cli.command));
}

#[test]
fn plan_run_still_attempt_auto_weave_upgrade() {
    let cli = Cli::parse_from([
        "csa",
        "plan",
        "run",
        "patterns/mktd/workflow.toml",
        "--sa-mode",
        "false",
    ]);
    assert!(should_attempt_auto_weave_upgrade(&cli.command));
}
