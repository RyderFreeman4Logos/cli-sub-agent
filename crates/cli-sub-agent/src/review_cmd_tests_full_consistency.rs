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
            resolved_pattern: None,
            prior_rounds_section: None,
            current_session_id: None,
            full_consistency: false,
            review_depth: crate::cli::ReviewDepth::Standard,
            review_depth_auto_escalation: None,
            regression_context: None,
        },
    );

    assert!(result.contains("scope=uncommitted"));
    assert!(result.contains("consistency_scope=diff-only"));
    assert!(result.contains("review_depth=standard"));
    assert!(result.contains("shell_semantics"));
    assert!(result.contains("subprocess_timeout"));
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
fn test_build_review_instruction_audit_depth_metadata() {
    let project_dir = tempdir().unwrap();
    let (result, _routing) = build_review_instruction_for_project(
        "uncommitted",
        "review-only",
        "on",
        ReviewMode::RedTeam,
        None,
        project_dir.path(),
        resolve::ReviewProjectPromptOptions {
            project_config: None,
            resolved_pattern: None,
            prior_rounds_section: None,
            current_session_id: None,
            full_consistency: false,
            review_depth: crate::cli::ReviewDepth::Audit,
            review_depth_auto_escalation: Some("Command::output usage".to_string()),
            regression_context: Some(
                "Recent commit history for changed files (regression context):\n- src/lib.rs:\n  abc1234 fix old bug",
            ),
        },
    );

    assert!(result.contains("review_mode=red-team"));
    assert!(result.contains("review_depth=audit"));
    assert!(result.contains("review_depth_auto_escalated=true (Command::output usage)"));
    assert!(result.contains("Audit depth is active: red-team review mode is enabled"));
    assert!(result.contains("<review-regression-context inert=\"true\">"));
    assert!(result.contains("Recent commit history for changed files (regression context):"));
    assert!(result.contains("</review-regression-context>"));
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
            resolved_pattern: None,
            prior_rounds_section: None,
            current_session_id: None,
            full_consistency: true,
            review_depth: crate::cli::ReviewDepth::Standard,
            review_depth_auto_escalation: None,
            regression_context: None,
        },
    );

    assert!(result.contains("scope=uncommitted"));
    assert!(result.contains("consistency_scope=touched-files"));
}
