use super::*;
use crate::review_prior_rounds::{
    PRIOR_ROUNDS_SECTION_HEADING, REVIEW_FINDINGS_TOML_INSTRUCTION, load_prior_rounds_section,
};
use csa_core::types::ToolName;
use tempfile::tempdir;

#[test]
fn parse_review_args_accepts_prior_rounds_summary_flag() {
    let args = parse_review_args(&[
        "csa",
        "review",
        "--prior-rounds-summary",
        "prior_rounds.toml",
    ]);

    assert_eq!(
        args.prior_rounds_summary.as_deref(),
        Some(std::path::Path::new("prior_rounds.toml"))
    );
}

#[test]
fn prior_rounds_summary_valid_toml_renders_rounds_in_single_and_multi_reviewer_prompts() {
    let project_dir = tempdir().unwrap();
    let summary_path = project_dir.path().join("prior_rounds.toml");
    std::fs::write(
        &summary_path,
        r#"
[[round]]
number = 6
commit = "29b6c34c"
summary = "narrowed legacy ACP fallback to tool == \"codex\""
invariant = "ACP codex sessions route to output.log"

[[round]]
number = 7
commit = "2fdcba62"
summary = "moved runtime_binary write behind lock"
invariant = "lock-losing resume cannot mutate metadata of winning session"
"#,
    )
    .unwrap();

    let prior_rounds = load_prior_rounds_section(&summary_path).expect("parse prior rounds");
    let (instruction, _routing) = build_review_instruction_for_project(
        "range:main...HEAD",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
        project_dir.path(),
        resolve::ReviewProjectPromptOptions {
            project_config: None,
            prior_rounds_section: Some(&prior_rounds),
            full_consistency: false,
        },
    );

    assert!(instruction.contains("## Prior-Round Invariant Verification"));
    assert!(instruction.contains("Round 6 (commit 29b6c34c)"));
    assert!(instruction.contains("ACP codex sessions route to output.log"));
    assert!(instruction.contains("Round 7 (commit 2fdcba62)"));
    assert!(instruction.contains("lock-losing resume cannot mutate metadata of winning session"));

    let single_idx = instruction
        .find("## Prior-Round Invariant Verification")
        .expect("single prompt must contain prior-round section");
    let single_findings_idx = instruction
        .find(REVIEW_FINDINGS_TOML_INSTRUCTION)
        .expect("single prompt must contain findings instruction");
    assert!(
        single_idx < single_findings_idx,
        "prior-round section must appear before findings instruction"
    );

    let prompt = crate::review_consensus::build_multi_reviewer_instruction(
        "Base prompt",
        2,
        ToolName::Codex,
        project_dir.path(),
        Some(&prior_rounds),
    );
    assert!(prompt.contains("Round 6 (commit 29b6c34c)"));
    assert!(prompt.contains("ACP codex sessions route to output.log"));
    let multi_idx = prompt
        .find("## Prior-Round Invariant Verification")
        .expect("multi prompt must contain prior-round section");
    let multi_findings_idx = prompt
        .find(REVIEW_FINDINGS_TOML_INSTRUCTION)
        .expect("multi prompt must contain findings instruction");
    assert!(
        multi_idx < multi_findings_idx,
        "prior-round section must appear before findings instruction"
    );
}

#[test]
fn prior_rounds_summary_missing_file_returns_clear_error() {
    let project_dir = tempdir().unwrap();
    let missing_path = project_dir.path().join("missing_prior_rounds.toml");
    let error = load_prior_rounds_section(&missing_path).expect_err("missing file must fail");

    assert!(
        error
            .to_string()
            .contains("Failed to read prior-rounds summary file")
    );
    assert!(error.to_string().contains("missing_prior_rounds.toml"));
}

#[test]
fn prior_rounds_summary_malformed_toml_returns_clear_error() {
    let project_dir = tempdir().unwrap();
    let summary_path = project_dir.path().join("prior_rounds.toml");
    std::fs::write(
        &summary_path,
        r#"
[[round]]
number = "oops"
commit = "29b6c34c"
"#,
    )
    .unwrap();

    let error = load_prior_rounds_section(&summary_path).expect_err("bad TOML must fail");
    assert!(
        error
            .to_string()
            .contains("Failed to parse prior-rounds summary TOML")
    );
    assert!(error.to_string().contains("prior_rounds.toml"));
}

#[test]
fn build_review_instruction_for_project_without_prior_rounds_flag_leaves_section_absent() {
    let project_dir = tempdir().unwrap();
    let (instruction, _routing) = build_review_instruction_for_project(
        "range:main...HEAD",
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

    assert!(!instruction.contains("## Prior-Round Invariant Verification"));
    assert!(instruction.contains(REVIEW_FINDINGS_TOML_INSTRUCTION));
}

#[test]
fn multi_reviewer_prompt_does_not_duplicate_prior_rounds_section_from_base_prompt() {
    let prior_rounds = "## Prior-Round Invariant Verification\n\nRound 6 (commit 29b6c34c): summary - Invariant: invariant";
    let base_prompt = format!("Base prompt\n\n{prior_rounds}");

    let prompt = crate::review_consensus::build_multi_reviewer_instruction(
        &base_prompt,
        2,
        ToolName::Codex,
        tempdir().unwrap().path(),
        Some(prior_rounds),
    );

    assert_eq!(prompt.matches(PRIOR_ROUNDS_SECTION_HEADING).count(), 1);
}
