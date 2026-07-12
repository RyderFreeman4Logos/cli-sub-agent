use super::output::{detect_tool_diagnostic, is_review_output_empty};
use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::{ProjectProfile, ToolRestrictions};
use std::path::Path;
use tempfile::tempdir;

#[path = "review_cmd_tests_scope_tail.rs"]
mod scope_tail_tests;

// --- is_worktree_submodule tests ---

#[test]
fn worktree_submodule_detected_when_gitdir_has_both_markers() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join(".git"),
        "gitdir: ../../../../.git/worktrees/my-wt/modules/crates/my-sub\n",
    )
    .unwrap();
    assert!(is_worktree_submodule(dir.path()));
}

#[test]
fn worktree_submodule_not_detected_for_normal_repo() {
    let dir = tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    assert!(!is_worktree_submodule(dir.path()));
}

#[test]
fn worktree_submodule_not_detected_for_plain_submodule() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join(".git"),
        "gitdir: ../../.git/modules/crates/my-sub\n",
    )
    .unwrap();
    assert!(!is_worktree_submodule(dir.path()));
}

#[test]
fn worktree_submodule_not_detected_for_plain_worktree() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join(".git"),
        "gitdir: /repo/.git/worktrees/my-wt\n",
    )
    .unwrap();
    assert!(!is_worktree_submodule(dir.path()));
}

// --- is_review_output_empty tests ---

#[test]
fn empty_string_is_empty_output() {
    assert!(is_review_output_empty(""));
}

#[test]
fn whitespace_only_is_empty_output() {
    assert!(is_review_output_empty("  \n  \n  "));
}

#[test]
fn prompt_guards_only_is_empty_output() {
    let output = r#"[csa-hook] reverse prompt injection for caller (guards=2)
<csa-caller-prompt-injection guards="2">
<prompt-guard name="branch-context">
BRANCH CONTEXT: The caller is on feature branch 'fix/test'.
</prompt-guard>
<prompt-guard name="commit-workflow">
WORKFLOW: 2 unpushed commit(s) on 'fix/test'.
</prompt-guard>
</csa-caller-prompt-injection>
"#;
    assert!(is_review_output_empty(output));
}

#[test]
fn substantive_output_is_not_empty() {
    let output =
        "## Review Findings\n\n1. Missing error handling in foo.rs:42\n\nVerdict: HAS_ISSUES";
    assert!(!is_review_output_empty(output));
}

#[test]
fn empty_section_wrappers_is_empty_output() {
    let output = "<!-- CSA:SECTION:summary -->\n<!-- CSA:SECTION:summary:END -->\n\
                  <!-- CSA:SECTION:details -->\n<!-- CSA:SECTION:details:END -->\n";
    assert!(is_review_output_empty(output));
}

#[test]
fn prompt_guards_plus_content_is_not_empty() {
    let output = r#"[csa-hook] reverse prompt injection for caller (guards=1)
<csa-caller-prompt-injection guards="1">
<prompt-guard name="branch-context">
BRANCH CONTEXT: feature branch
</prompt-guard>
</csa-caller-prompt-injection>
## Review Findings
No issues found. Verdict: CLEAN
"#;
    assert!(!is_review_output_empty(output));
}

// --- detect_tool_diagnostic tests ---

#[test]
fn test_detect_tool_diagnostic_empty_strings_returns_none() {
    assert!(detect_tool_diagnostic("", "").is_none());
}

#[test]
fn test_detect_tool_diagnostic_normal_output_returns_none() {
    let stdout = "## Review Findings\n\nNo issues found.\n\nVerdict: CLEAN";
    let stderr = "Loaded 3 MCP servers\n";
    assert!(detect_tool_diagnostic(stdout, stderr).is_none());
}

#[test]
fn test_detect_tool_diagnostic_mcp_issues_in_stdout_returns_diagnostic() {
    let stdout = "Some output\nMCP issues detected\nMore output";
    let result = detect_tool_diagnostic(stdout, "");
    assert!(result.is_some());
    let msg = result.unwrap();
    assert!(msg.contains("MCP init degraded"));
    assert!(msg.contains("csa doctor"));
    assert!(msg.contains("--force-ignore-tier-setting"));
}

#[test]
fn test_detect_tool_diagnostic_mcp_issues_in_stderr_returns_diagnostic() {
    let stderr = "Warning: MCP issues detected during startup";
    let result = detect_tool_diagnostic("", stderr);
    assert!(result.is_some());
    assert!(result.unwrap().contains("MCP init degraded"));
}

#[test]
fn test_detect_tool_diagnostic_mcp_list_in_stderr_returns_diagnostic() {
    let stderr = "Run /mcp list to check server status";
    let result = detect_tool_diagnostic("", stderr);
    assert!(result.is_some());
    assert!(result.unwrap().contains("csa doctor"));
}

#[test]
fn detect_tool_diagnostic_returns_quota_message_for_gemini_429() {
    let stderr = r#"{"code": 429, "reason": "QUOTA_EXHAUSTED", "message": "You have exhausted your capacity on this model. Your quota will reset after 16h8m32s."}"#;
    let result = detect_tool_diagnostic("", stderr);
    assert!(result.is_some());
    let msg = result.unwrap();
    assert!(msg.contains("provider quota exhausted"), "got: {msg}");
    assert!(msg.contains("supported provider API key"), "got: {msg}");
}

#[test]
fn detect_tool_diagnostic_prefers_quota_over_mcp_when_both_present() {
    let stderr = "MCP issues detected. Run /mcp list for status.\ncause: {code: 429, reason: 'QUOTA_EXHAUSTED'}";
    let result = detect_tool_diagnostic("", stderr);
    assert!(result.is_some());
    let msg = result.unwrap();
    assert!(msg.contains("provider quota exhausted"), "got: {msg}");
    assert!(
        !msg.contains("Run `csa doctor`"),
        "should not emit MCP guidance when quota is root cause; got: {msg}"
    );
}

#[test]
fn detect_tool_diagnostic_still_returns_mcp_when_only_mcp_markers() {
    let stderr = "MCP issues detected. Run /mcp list for status.";
    let result = detect_tool_diagnostic("", stderr);
    assert!(result.is_some());
    let msg = result.unwrap();
    assert!(msg.contains("MCP init degraded"), "got: {msg}");
}

#[test]
fn test_detect_tool_diagnostic_unrelated_mcp_text_returns_none() {
    let stdout = "MCP servers loaded successfully";
    let stderr = "Connected to 2 MCP backends";
    assert!(detect_tool_diagnostic(stdout, stderr).is_none());
}

#[test]
fn test_is_review_output_empty_with_mcp_diagnostic_text_is_not_empty() {
    let output = "MCP issues detected\nRun /mcp list to check";
    assert!(!is_review_output_empty(output));
}

// --- review tier precedence tests ---

#[test]
fn resolve_review_model_prefers_cli_over_config() {
    let model = resolve_review_model(Some("sonnet"), Some("opus"), false);
    assert_eq!(model.as_deref(), Some("sonnet"));
}

#[test]
fn resolve_review_model_ignores_config_when_tier_active() {
    let model = resolve_review_model(None, Some("opus"), true);
    assert_eq!(model, None);
}

#[test]
fn resolve_review_tool_prefers_model_spec_override() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["codex"]);
    let tool = resolve_review_tool(
        None,
        Some("codex/openai/gpt-5.4/xhigh"),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,
        false,
    )
    .unwrap();
    assert!(matches!(tool.0, ToolName::Codex));
    assert_eq!(tool.1.as_deref(), Some("codex/openai/gpt-5.4/xhigh"));
}

#[test]
fn resolve_review_thinking_uses_config_without_tier() {
    let thinking = resolve_review_thinking(None, Some("high"), false);
    assert_eq!(thinking.as_deref(), Some("high"));
}

#[test]
fn resolve_review_thinking_ignores_config_when_tier_active() {
    let thinking = resolve_review_thinking(None, Some("high"), true);
    assert_eq!(thinking, None);
}

#[test]
fn review_cli_parses_model_spec_and_no_failover_flags() {
    let args = parse_review_args(&[
        "csa",
        "review",
        "--diff",
        "--model-spec",
        "codex/openai/gpt-5.4/xhigh",
        "--no-failover",
    ]);
    assert_eq!(
        args.model_spec.as_deref(),
        Some("codex/openai/gpt-5.4/xhigh")
    );
    assert!(args.no_failover);
}

// --- --thinking silent acceptance tests ---

#[test]
fn thinking_flag_accepted_silently() {
    let args = parse_review_args(&["csa", "review", "--thinking", "xhigh", "--diff"]);
    assert_eq!(args.thinking.as_deref(), Some("xhigh"));
    assert!(args.diff);
}

#[test]
fn thinking_flag_optional() {
    let args = parse_review_args(&["csa", "review", "--diff"]);
    assert!(args.thinking.is_none());
}

// --- verify_review_skill_available tests ---

#[test]
fn review_cli_parses_explicit_review_mode() {
    let args = parse_review_args(&["csa", "review", "--diff", "--review-mode", "red-team"]);
    assert_eq!(args.effective_review_mode(), ReviewMode::RedTeam);
    assert_eq!(args.effective_security_mode(), "on");
}

#[test]
fn review_cli_parses_red_team_shorthand() {
    let args = parse_review_args(&["csa", "review", "--red-team", "--diff"]);
    assert!(args.red_team);
    assert_eq!(args.effective_review_mode(), ReviewMode::RedTeam);
    assert_eq!(args.effective_security_mode(), "on");
}

#[test]
fn review_cli_rejects_red_team_with_security_off() {
    let err = parse_or_validate_review_error(&[
        "csa",
        "review",
        "--red-team",
        "--diff",
        "--security-mode",
        "off",
    ]);
    assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
}

#[test]
fn review_cli_rejects_review_mode_red_team_with_security_off() {
    let err = parse_or_validate_review_error(&[
        "csa",
        "review",
        "--review-mode",
        "red-team",
        "--diff",
        "--security-mode",
        "off",
    ]);
    assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
}

// --- build_review_instruction tests ---

#[test]
fn test_build_review_instruction_basic() {
    let result = build_review_instruction(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
    );
    assert!(result.contains("scope=uncommitted"));
    assert!(result.contains("mode=review-only"));
    assert!(result.contains("security_mode=auto"));
    assert!(result.contains("review_mode=standard"));
    assert!(result.contains("csa-review skill"));
    assert!(!result.contains("git diff"));
    assert!(!result.contains("Pass 1:"));
}

#[test]
fn test_build_review_instruction_with_context() {
    let context = ResolvedReviewContext {
        path: "/path/to/TODO.md".to_string(),
        kind: ResolvedReviewContextKind::TodoMarkdown,
    };
    let result = build_review_instruction(
        "range:main...HEAD",
        "review-only",
        "on",
        ReviewMode::Standard,
        Some(&context),
    );
    assert!(result.contains("scope=range:main...HEAD"));
    assert!(result.contains("context=/path/to/TODO.md"));
}

#[test]
fn test_build_review_instruction_fix_mode() {
    let result = build_review_instruction(
        "uncommitted",
        "review-and-fix",
        "auto",
        ReviewMode::Standard,
        None,
    );
    assert!(result.contains("mode=review-and-fix"));
}

#[test]
fn test_build_review_instruction_no_diff_content() {
    let result = build_review_instruction(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
    );
    assert!(
        result.len() < 3000,
        "Instruction should stay bounded even with review anchors, got {} chars",
        result.len()
    );
    assert!(!result.contains("git diff"));
    assert!(!result.contains("Pass 1:"));
    assert!(result.contains("Design preferences vs correctness bugs"));
    assert!(result.contains("Bounded same-class site sweep"));
}

#[test]
fn test_build_review_instruction_for_project_includes_rust_profile() {
    let project_dir = tempdir().unwrap();
    std::fs::write(
        project_dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let (instruction, routing) = build_review_instruction_for_project(
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

    assert!(instruction.contains("[project_profile: rust]"));
    assert_eq!(routing.detection_method, "auto");
}

#[test]
fn test_build_review_instruction_for_project_includes_unknown_profile_for_empty_project() {
    let project_dir = tempdir().unwrap();

    let (instruction, routing) = build_review_instruction_for_project(
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

    assert!(instruction.contains("[project_profile: unknown]"));
    assert_eq!(routing.detection_method, "auto");
}

include!("review_cmd_tests_tail_part2.rs");
