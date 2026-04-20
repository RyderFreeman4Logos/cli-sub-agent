#[path = "../src/cli.rs"]
mod cli_defs;

#[allow(dead_code)]
#[path = "../src/bug_class.rs"]
mod bug_class;
#[allow(dead_code)]
#[path = "../src/test_session_sandbox.rs"]
mod test_session_sandbox;

#[path = "../src/review_consensus.rs"]
mod review_consensus;
#[path = "../src/review_design_anchor.rs"]
mod review_design_anchor;
#[allow(dead_code)]
#[path = "../src/review_prior_rounds.rs"]
mod review_prior_rounds;
#[allow(dead_code)]
#[path = "../src/test_env_lock.rs"]
mod test_env_lock;

use chrono::{TimeZone, Utc};
use clap::{Parser, error::ErrorKind};
use cli_defs::{Cli, Commands, ReviewMode};
use csa_core::{consensus::ConsensusStrategy, types::ToolName};
use csa_session::review_artifact::{ReviewArtifact, SeveritySummary};

fn sample_artifact(review_mode: Option<&str>, session_id: &str) -> ReviewArtifact {
    ReviewArtifact {
        findings: Vec::new(),
        severity_summary: SeveritySummary::default(),
        review_mode: review_mode.map(str::to_string),
        schema_version: "1.0".to_string(),
        session_id: session_id.to_string(),
        timestamp: Utc
            .with_ymd_and_hms(2026, 2, 24, 0, 0, 0)
            .single()
            .expect("valid timestamp"),
    }
}

#[test]
fn review_artifact_roundtrips_red_team_review_mode() {
    let artifact = sample_artifact(Some("red-team"), "session-red-team");

    let json = serde_json::to_string(&artifact).expect("artifact should serialize");
    let decoded: ReviewArtifact = serde_json::from_str(&json).expect("artifact should deserialize");

    assert_eq!(decoded.review_mode.as_deref(), Some("red-team"));
    assert_eq!(decoded, artifact);
}

#[test]
fn review_artifact_keeps_legacy_payloads_backward_compatible() {
    let json = r#"
    {
        "findings": [],
        "severity_summary": { "critical": 0, "high": 0, "medium": 0, "low": 0, "info": 0 },
        "schema_version": "1.0",
        "session_id": "session-legacy",
        "timestamp": "2026-02-24T00:00:00Z"
    }
    "#;

    let decoded: ReviewArtifact =
        serde_json::from_str(json).expect("legacy artifact should deserialize");

    assert_eq!(decoded.review_mode, None);
}

#[test]
fn consolidated_artifact_preserves_review_mode_from_reviewers() {
    let consolidated = review_consensus::build_consolidated_artifact(
        vec![
            sample_artifact(None, "session-standard"),
            sample_artifact(Some("red-team"), "session-red-team"),
        ],
        "session-final",
    );

    assert_eq!(consolidated.review_mode.as_deref(), Some("red-team"));
}

#[test]
fn review_mode_parses_standard_and_red_team_cli_args() {
    let standard = Cli::try_parse_from(["csa", "review", "--review-mode", "standard", "--diff"])
        .expect("standard review args should parse");
    let explicit_red_team =
        Cli::try_parse_from(["csa", "review", "--review-mode", "red-team", "--diff"])
            .expect("red-team review args should parse");
    let shorthand_red_team = Cli::try_parse_from(["csa", "review", "--red-team", "--diff"])
        .expect("red-team shorthand should parse");

    match standard.command {
        Commands::Review(args) => {
            assert_eq!(args.effective_review_mode(), ReviewMode::Standard);
            assert_eq!(args.effective_security_mode(), "auto");
        }
        _ => panic!("expected review command"),
    }

    match explicit_red_team.command {
        Commands::Review(args) => {
            assert_eq!(args.effective_review_mode(), ReviewMode::RedTeam);
            assert_eq!(args.effective_security_mode(), "on");
        }
        _ => panic!("expected review command"),
    }

    match shorthand_red_team.command {
        Commands::Review(args) => {
            assert_eq!(args.effective_review_mode(), ReviewMode::RedTeam);
            assert_eq!(args.effective_security_mode(), "on");
        }
        _ => panic!("expected review command"),
    }
}

#[test]
fn review_cli_validation_and_consensus_helpers_remain_compatible() {
    let cli = Cli::try_parse_from(["csa", "review", "--red-team", "--diff"])
        .expect("red-team review args should parse");
    cli_defs::validate_command_args(&cli.command, 1800)
        .expect("red-team review args should validate");
    let project_dir = tempfile::tempdir().expect("tempdir should be created");

    let instruction = review_consensus::build_multi_reviewer_instruction(
        "Base prompt",
        2,
        ToolName::Codex,
        project_dir.path(),
        None,
    );
    assert!(instruction.contains("reviewer 2"));
    assert!(instruction.contains("CLEAN"));
    assert!(instruction.contains("HAS_ISSUES"));
    assert!(instruction.contains("codex"));

    assert_eq!(
        review_consensus::consensus_strategy_label(ConsensusStrategy::Majority),
        "majority"
    );
}

#[test]
fn review_cli_validation_rejects_single_with_multiple_reviewers() {
    let cli = Cli::try_parse_from(["csa", "review", "--diff", "--single", "--reviewers", "2"])
        .expect("single flag should parse before validation");
    let err = match cli.command {
        Commands::Review(args) => {
            cli_defs::validate_review_args(&args).expect_err("validation should reject conflict")
        }
        _ => panic!("expected review command"),
    };

    assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
}
