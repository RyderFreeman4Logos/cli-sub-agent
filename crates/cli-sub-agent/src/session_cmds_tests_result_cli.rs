use crate::cli::{Cli, Commands, SessionCommands};
use clap::Parser;

#[test]
fn session_result_cli_parses_summary_flag() {
    let cli = Cli::try_parse_from([
        "csa",
        "session",
        "result",
        "--session",
        "01ABCDEF",
        "--summary",
    ])
    .unwrap();
    match cli.command {
        Commands::Session {
            cmd:
                SessionCommands::Result {
                    summary,
                    section,
                    full,
                    ..
                },
        } => {
            assert!(summary);
            assert!(section.is_none());
            assert!(!full);
        }
        _ => panic!("expected session result command"),
    }
}

#[test]
fn session_result_cli_parses_section_flag() {
    let cli = Cli::try_parse_from([
        "csa",
        "session",
        "result",
        "--session",
        "01ABCDEF",
        "--section",
        "details",
    ])
    .unwrap();
    match cli.command {
        Commands::Session {
            cmd:
                SessionCommands::Result {
                    summary,
                    section,
                    full,
                    ..
                },
        } => {
            assert!(!summary);
            assert_eq!(section.as_deref(), Some("details"));
            assert!(!full);
        }
        _ => panic!("expected session result command"),
    }
}

#[test]
fn session_result_cli_parses_full_flag() {
    let cli = Cli::try_parse_from([
        "csa",
        "session",
        "result",
        "--session",
        "01ABCDEF",
        "--full",
    ])
    .unwrap();
    match cli.command {
        Commands::Session {
            cmd:
                SessionCommands::Result {
                    summary,
                    section,
                    full,
                    ..
                },
        } => {
            assert!(!summary);
            assert!(section.is_none());
            assert!(full);
        }
        _ => panic!("expected session result command"),
    }
}

#[test]
fn session_result_cli_rejects_conflicting_flags() {
    // --summary and --full conflict
    let result = Cli::try_parse_from([
        "csa",
        "session",
        "result",
        "-s",
        "01ABC",
        "--summary",
        "--full",
    ]);
    assert!(result.is_err());

    // --summary and --section conflict
    let result = Cli::try_parse_from([
        "csa",
        "session",
        "result",
        "-s",
        "01ABC",
        "--summary",
        "--section",
        "x",
    ]);
    assert!(result.is_err());

    // --section and --full conflict
    let result = Cli::try_parse_from([
        "csa",
        "session",
        "result",
        "-s",
        "01ABC",
        "--section",
        "x",
        "--full",
    ]);
    assert!(result.is_err());
}

#[test]
fn session_result_cli_defaults_no_structured_flags() {
    let cli = Cli::try_parse_from(["csa", "session", "result", "--session", "01ABCDEF"]).unwrap();
    match cli.command {
        Commands::Session {
            cmd:
                SessionCommands::Result {
                    summary,
                    section,
                    full,
                    json,
                    ..
                },
        } => {
            assert!(!summary);
            assert!(section.is_none());
            assert!(!full);
            assert!(!json);
        }
        _ => panic!("expected session result command"),
    }
}
