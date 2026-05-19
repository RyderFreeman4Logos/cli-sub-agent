use super::*;
use crate::debate_cmd_output::{DebateOutputHeader, DebateSummary, format_debate_stdout_text};
use csa_core::types::ToolName;

#[test]
fn format_debate_stdout_text_includes_prompt_bytes_header() {
    let summary = DebateSummary {
        verdict: "PASS".to_string(),
        decision: None,
        confidence: "high".to_string(),
        summary: "Prompt size is visible.".to_string(),
        key_points: vec![],
        failure_reason: None,
        mode: DebateMode::Heterogeneous,
    };

    let text =
        format_debate_stdout_text(&summary, "", Some(DebateOutputHeader { prompt_bytes: 42 }));

    assert!(text.starts_with("Debate prompt bytes: 42\nDebate verdict: PASS"));
}

#[test]
fn debate_cli_parses_dry_run_flag() {
    let args = parse_debate_args(&["csa", "debate", "--dry-run", "question"]);
    assert!(args.dry_run);
}

#[test]
fn debate_explicit_tool_keeps_failover_enabled_by_default() {
    let args = parse_debate_args(&["csa", "debate", "--tool", "gemini-cli", "question"]);
    assert_eq!(args.tool, Some(ToolName::GeminiCli));
    assert!(!args.no_failover);
}
