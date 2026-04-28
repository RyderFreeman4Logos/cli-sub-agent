use super::*;
use crate::cli::{Cli, Commands, ReviewMode, validate_review_args};
use clap::Parser;
use tempfile::tempdir;

fn parse_review_args(argv: &[&str]) -> ReviewArgs {
    let cli = Cli::try_parse_from(argv).expect("review CLI args should parse");
    match cli.command {
        Commands::Review(args) => {
            validate_review_args(&args).expect("review CLI args should validate");
            args
        }
        _ => panic!("expected review subcommand"),
    }
}

#[test]
fn review_cli_parses_full_consistency_flag() {
    let args = parse_review_args(&["csa", "review", "--diff", "--full-consistency"]);

    assert!(args.diff);
    assert!(args.full_consistency);
}

#[test]
fn test_build_review_instruction_no_diff_content_default() {
    let project_dir = tempdir().unwrap();
    let (result, _routing) = build_review_instruction_for_project(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
        project_dir.path(),
        resolve::ReviewProjectPromptOptions {
            project_config: None,
            prior_rounds_section: None,
            full_consistency: false,
        },
    );

    assert!(result.contains("scope=uncommitted"));
    assert!(result.contains("consistency_scope=diff-only"));
    assert!(
        result
            .find("consistency_scope=diff-only")
            .expect("consistency scope marker")
            < result
                .find("Design preferences vs correctness bugs")
                .expect("design anchor")
    );
    assert!(!result.contains("git diff"));
    assert!(!result.contains("Pass 1:"));
}

#[test]
fn test_build_review_instruction_full_consistency() {
    let project_dir = tempdir().unwrap();
    let (result, _routing) = build_review_instruction_for_project(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
        project_dir.path(),
        resolve::ReviewProjectPromptOptions {
            project_config: None,
            prior_rounds_section: None,
            full_consistency: true,
        },
    );

    assert!(result.contains("scope=uncommitted"));
    assert!(result.contains("consistency_scope=touched-files"));
}
