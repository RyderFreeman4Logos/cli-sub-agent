use clap::Parser;

use crate::cli::{Cli, Commands, DebateArgs};

fn parse_debate_args(argv: &[&str]) -> DebateArgs {
    let cli = Cli::try_parse_from(argv).expect("debate CLI args should parse");
    match cli.command {
        Commands::Debate(args) => args,
        _ => panic!("expected debate subcommand"),
    }
}

#[test]
fn debate_cli_accepts_resource_override_flags() {
    let args = parse_debate_args(&[
        "csa",
        "debate",
        "--memory-max-mb",
        "6144",
        "--min-free-memory-mb",
        "256",
        "question",
    ]);

    assert_eq!(args.memory_max_mb, Some(6144));
    assert_eq!(args.min_free_memory_mb, Some(256));
}

#[test]
fn debate_cli_rejects_memory_override_below_config_minimum() {
    let result = Cli::try_parse_from(["csa", "debate", "--memory-max-mb", "255", "question"]);

    let err = match result {
        Ok(_) => panic!("memory override below configured minimum should fail"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
}
