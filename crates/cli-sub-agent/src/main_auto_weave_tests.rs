use super::{Cli, should_attempt_auto_weave_upgrade};
use clap::Parser;

#[test]
fn todo_commands_skip_auto_weave_upgrade() {
    let cli = Cli::parse_from(["csa", "todo", "list"]);
    assert!(!should_attempt_auto_weave_upgrade(&cli.command));
}

#[test]
fn session_commands_skip_auto_weave_upgrade() {
    let cli = Cli::parse_from(["csa", "session", "list"]);
    assert!(!should_attempt_auto_weave_upgrade(&cli.command));
}

#[test]
fn config_commands_skip_auto_weave_upgrade() {
    let cli = Cli::parse_from(["csa", "config", "show"]);
    assert!(!should_attempt_auto_weave_upgrade(&cli.command));
}

#[test]
fn doctor_skips_auto_weave_upgrade() {
    let cli = Cli::parse_from(["csa", "doctor"]);
    assert!(!should_attempt_auto_weave_upgrade(&cli.command));
}

#[test]
fn gc_skips_auto_weave_upgrade() {
    let cli = Cli::parse_from(["csa", "gc"]);
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

#[test]
fn review_still_attempts_auto_weave_upgrade() {
    let cli = Cli::parse_from(["csa", "review", "--sa-mode", "false", "--diff"]);
    assert!(should_attempt_auto_weave_upgrade(&cli.command));
}

#[test]
fn debate_still_attempts_auto_weave_upgrade() {
    let cli = Cli::parse_from(["csa", "debate", "--sa-mode", "false", "question"]);
    assert!(should_attempt_auto_weave_upgrade(&cli.command));
}
