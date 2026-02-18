use super::*;
use crate::cli::{Cli, Commands};
use clap::Parser;
use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig};
use std::collections::HashMap;

fn project_config_with_enabled_tools(tools: &[&str]) -> ProjectConfig {
    let mut tool_map = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        tool_map.insert(
            tool.as_str().to_string(),
            ToolConfig {
                enabled: false,
                restrictions: None,
                suppress_notify: true,
            },
        );
    }
    for tool in tools {
        tool_map.insert(
            (*tool).to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
                suppress_notify: true,
            },
        );
    }

    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        tools: tool_map,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
    }
}

fn parse_review_args(argv: &[&str]) -> ReviewArgs {
    let cli = Cli::try_parse_from(argv).expect("review CLI args should parse");
    match cli.command {
        Commands::Review(args) => args,
        _ => panic!("expected review subcommand"),
    }
}

// --- resolve_review_tool tests ---

#[test]
fn resolve_review_tool_prefers_cli_override() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
    let tool = resolve_review_tool(
        Some(ToolName::Codex),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap();
    assert!(matches!(tool, ToolName::Codex));
}

#[test]
fn resolve_review_tool_uses_global_review_config_with_parent_tool() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
    let tool = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap();
    assert!(matches!(tool, ToolName::Codex));
}

#[test]
fn resolve_review_tool_errors_without_parent_tool_context() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["opencode"]);
    let err = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        None,
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("AUTO review tool selection failed")
    );
}

#[test]
fn resolve_review_tool_errors_on_invalid_explicit_global_tool() {
    let mut global = GlobalConfig::default();
    global.review.tool = "invalid-tool".to_string();
    let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
    let err = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        Some("codex"),
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("Invalid [review].tool value 'invalid-tool'")
    );
}

#[test]
fn resolve_review_tool_prefers_project_override() {
    let global = GlobalConfig::default();
    let mut cfg = project_config_with_enabled_tools(&["codex", "opencode"]);
    cfg.review = Some(csa_config::global::ReviewConfig {
        tool: "opencode".to_string(),
    });
    cfg.debate = None;

    let tool = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap();
    assert!(matches!(tool, ToolName::Opencode));
}

#[test]
fn resolve_review_tool_project_auto_maps_to_heterogeneous_counterpart() {
    let global = GlobalConfig::default();
    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code"]);
    cfg.review = Some(csa_config::global::ReviewConfig {
        tool: "auto".to_string(),
    });
    cfg.debate = None;

    let tool = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap();
    assert!(matches!(tool, ToolName::Codex));
}

#[test]
fn resolve_review_tool_project_auto_prefers_priority_over_counterpart() {
    let mut global = GlobalConfig::default();
    global.preferences.tool_priority = vec!["opencode".to_string(), "claude-code".to_string()];

    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
    cfg.review = Some(csa_config::global::ReviewConfig {
        tool: "auto".to_string(),
    });
    cfg.debate = None;

    let tool = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        Some("codex"),
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap();
    assert!(matches!(tool, ToolName::Opencode));
}

#[test]
fn resolve_review_tool_ignores_unknown_priority_entries() {
    let mut global = GlobalConfig::default();
    global.preferences.tool_priority = vec!["codexx".to_string()];

    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
    cfg.review = Some(csa_config::global::ReviewConfig {
        tool: "auto".to_string(),
    });
    cfg.debate = None;

    let tool = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        Some("codex"),
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap();
    assert!(matches!(tool, ToolName::ClaudeCode));
}

// --- derive_scope tests ---

#[test]
fn derive_scope_uncommitted() {
    let args = ReviewArgs {
        tool: None,
        session: None,
        model: None,
        diff: true,
        branch: "main".to_string(),
        commit: None,
        range: None,
        files: None,
        fix: false,
        security_mode: "auto".to_string(),
        context: None,
        reviewers: 1,
        consensus: "majority".to_string(),
        cd: None,
        timeout: None,
        idle_timeout: None,
        stream_stdout: false,
        no_stream_stdout: false,
    };
    assert_eq!(derive_scope(&args), "uncommitted");
}

#[test]
fn derive_scope_commit() {
    let args = ReviewArgs {
        tool: None,
        session: None,
        model: None,
        diff: false,
        branch: "main".to_string(),
        commit: Some("abc123".to_string()),
        range: None,
        files: None,
        fix: false,
        security_mode: "auto".to_string(),
        context: None,
        reviewers: 1,
        consensus: "majority".to_string(),
        cd: None,
        timeout: None,
        idle_timeout: None,
        stream_stdout: false,
        no_stream_stdout: false,
    };
    assert_eq!(derive_scope(&args), "commit:abc123");
}

#[test]
fn derive_scope_range() {
    let args = ReviewArgs {
        tool: None,
        session: None,
        model: None,
        diff: false,
        branch: "main".to_string(),
        commit: None,
        range: Some("main...HEAD".to_string()),
        files: None,
        fix: false,
        security_mode: "auto".to_string(),
        context: None,
        reviewers: 1,
        consensus: "majority".to_string(),
        cd: None,
        timeout: None,
        idle_timeout: None,
        stream_stdout: false,
        no_stream_stdout: false,
    };
    assert_eq!(derive_scope(&args), "range:main...HEAD");
}

#[test]
fn derive_scope_files() {
    let args = ReviewArgs {
        tool: None,
        session: None,
        model: None,
        diff: false,
        branch: "main".to_string(),
        commit: None,
        range: None,
        files: Some("src/**/*.rs".to_string()),
        fix: false,
        security_mode: "auto".to_string(),
        context: None,
        reviewers: 1,
        consensus: "majority".to_string(),
        cd: None,
        timeout: None,
        idle_timeout: None,
        stream_stdout: false,
        no_stream_stdout: false,
    };
    assert_eq!(derive_scope(&args), "files:src/**/*.rs");
}

#[test]
fn derive_scope_default_branch() {
    let args = ReviewArgs {
        tool: None,
        session: None,
        model: None,
        diff: false,
        branch: "develop".to_string(),
        commit: None,
        range: None,
        files: None,
        fix: false,
        security_mode: "auto".to_string(),
        context: None,
        reviewers: 1,
        consensus: "majority".to_string(),
        cd: None,
        timeout: None,
        idle_timeout: None,
        stream_stdout: false,
        no_stream_stdout: false,
    };
    assert_eq!(derive_scope(&args), "base:develop");
}

#[test]
fn derive_scope_range_takes_priority_over_commit() {
    let args = ReviewArgs {
        tool: None,
        session: None,
        model: None,
        diff: true,
        branch: "main".to_string(),
        commit: Some("abc".to_string()),
        range: Some("v1...v2".to_string()),
        files: None,
        fix: false,
        security_mode: "auto".to_string(),
        context: None,
        reviewers: 1,
        consensus: "majority".to_string(),
        cd: None,
        timeout: None,
        idle_timeout: None,
        stream_stdout: false,
        no_stream_stdout: false,
    };
    // --range has highest priority
    assert_eq!(derive_scope(&args), "range:v1...v2");
}

#[test]
fn review_cli_parses_range_scope_with_multiple_reviewers() {
    let args = parse_review_args(&[
        "csa",
        "review",
        "--range",
        "main...HEAD",
        "--reviewers",
        "3",
    ]);

    assert_eq!(args.reviewers, 3);
    assert_eq!(derive_scope(&args), "range:main...HEAD");
}

#[test]
fn review_cli_parses_weighted_consensus_for_multi_reviewer_mode() {
    let args = parse_review_args(&[
        "csa",
        "review",
        "--diff",
        "--reviewers",
        "2",
        "--consensus",
        "weighted",
    ]);

    let strategy = parse_consensus_strategy(&args.consensus).unwrap();
    assert_eq!(consensus_strategy_label(strategy), "weighted");
}

#[test]
fn review_cli_builds_multi_reviewer_config_from_args() {
    let args = parse_review_args(&[
        "csa",
        "review",
        "--tool",
        "codex",
        "--reviewers",
        "4",
        "--consensus",
        "unanimous",
    ]);

    let strategy = parse_consensus_strategy(&args.consensus).unwrap();
    let reviewers = args.reviewers as usize;
    let reviewer_tools = build_reviewer_tools(args.tool, ToolName::Codex, None, None, reviewers);

    assert!(reviewers > 1);
    assert_eq!(consensus_strategy_label(strategy), "unanimous");
    assert_eq!(reviewer_tools.len(), reviewers);
    assert!(reviewer_tools.iter().all(|tool| *tool == ToolName::Codex));
}

// --- build_review_instruction tests ---

#[test]
fn test_build_review_instruction_basic() {
    let result = build_review_instruction("uncommitted", "review-only", "auto", None);
    assert!(result.contains("scope=uncommitted"));
    assert!(result.contains("mode=review-only"));
    assert!(result.contains("security_mode=auto"));
    assert!(result.contains("csa-review skill"));
    // Must NOT contain review instructions or diff content
    assert!(!result.contains("git diff"));
    assert!(!result.contains("Pass 1:"));
}

#[test]
fn test_build_review_instruction_with_context() {
    let result = build_review_instruction(
        "range:main...HEAD",
        "review-only",
        "on",
        Some("/path/to/todo"),
    );
    assert!(result.contains("scope=range:main...HEAD"));
    assert!(result.contains("context=/path/to/todo"));
}

#[test]
fn test_build_review_instruction_fix_mode() {
    let result = build_review_instruction("uncommitted", "review-and-fix", "auto", None);
    assert!(result.contains("mode=review-and-fix"));
}

#[test]
fn test_build_review_instruction_no_diff_content() {
    // Critical: the instruction must not contain actual diff output or review protocol
    let result = build_review_instruction("uncommitted", "review-only", "auto", None);
    assert!(
        result.len() < 200,
        "Instruction should be concise, got {} chars",
        result.len()
    );
}

// --- CLI parse tests for timeout/stream flags (#146) ---

#[test]
fn review_cli_parses_timeout_flag() {
    let args = parse_review_args(&["csa", "review", "--diff", "--timeout", "120"]);
    assert_eq!(args.timeout, Some(120));
}

#[test]
fn review_cli_parses_idle_timeout_flag() {
    let args = parse_review_args(&["csa", "review", "--diff", "--idle-timeout", "60"]);
    assert_eq!(args.idle_timeout, Some(60));
}

#[test]
fn review_cli_parses_both_timeouts() {
    let args = parse_review_args(&[
        "csa",
        "review",
        "--diff",
        "--timeout",
        "300",
        "--idle-timeout",
        "30",
    ]);
    assert_eq!(args.timeout, Some(300));
    assert_eq!(args.idle_timeout, Some(30));
}

#[test]
fn review_cli_parses_stream_stdout_flag() {
    let args = parse_review_args(&["csa", "review", "--diff", "--stream-stdout"]);
    assert!(args.stream_stdout);
    assert!(!args.no_stream_stdout);
}

#[test]
fn review_cli_parses_no_stream_stdout_flag() {
    let args = parse_review_args(&["csa", "review", "--diff", "--no-stream-stdout"]);
    assert!(!args.stream_stdout);
    assert!(args.no_stream_stdout);
}

#[test]
fn review_cli_defaults_no_timeout() {
    let args = parse_review_args(&["csa", "review", "--diff"]);
    assert_eq!(args.timeout, None);
    assert_eq!(args.idle_timeout, None);
    assert!(!args.stream_stdout);
    assert!(!args.no_stream_stdout);
}

#[test]
fn review_cli_rejects_zero_timeout() {
    let result = Cli::try_parse_from(["csa", "review", "--diff", "--timeout", "0"]);
    assert!(result.is_err(), "timeout=0 should be rejected");
}

#[test]
fn review_cli_rejects_zero_idle_timeout() {
    let result = Cli::try_parse_from(["csa", "review", "--diff", "--idle-timeout", "0"]);
    assert!(result.is_err(), "idle_timeout=0 should be rejected");
}

// --- resolve_review_stream_mode tests ---

#[test]
fn stream_mode_default_non_tty_is_buffer_only() {
    // In test environment (non-TTY stderr), default should be BufferOnly
    let mode = resolve_review_stream_mode(false, false);
    // Note: in interactive TTY, default would be TeeToStderr (fixes #139)
    assert!(matches!(mode, csa_process::StreamMode::BufferOnly));
}

#[test]
fn stream_mode_explicit_stream() {
    let mode = resolve_review_stream_mode(true, false);
    assert!(matches!(mode, csa_process::StreamMode::TeeToStderr));
}

#[test]
fn stream_mode_explicit_no_stream() {
    let mode = resolve_review_stream_mode(false, true);
    assert!(matches!(mode, csa_process::StreamMode::BufferOnly));
}

// --- verify_review_skill_available tests ---

#[test]
fn verify_review_skill_missing_returns_actionable_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let err = verify_review_skill_available(tmp.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Review pattern not found"),
        "should mention missing pattern: {msg}"
    );
    assert!(
        msg.contains("csa skill install"),
        "should include install guidance: {msg}"
    );
    assert!(
        msg.contains("patterns/csa-review"),
        "should list searched paths: {msg}"
    );
}

#[test]
fn verify_review_skill_present_succeeds() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Pattern layout: .csa/patterns/csa-review/skills/csa-review/SKILL.md
    let skill_dir = tmp
        .path()
        .join(".csa")
        .join("patterns")
        .join("csa-review")
        .join("skills")
        .join("csa-review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "# CSA Review Skill\nStructured code review.",
    )
    .unwrap();

    assert!(verify_review_skill_available(tmp.path()).is_ok());
}

#[test]
fn verify_review_skill_no_fallback_without_skill() {
    // Ensure no execution path silently downgrades when skill is missing.
    // The verify function must return Err — it must NOT return Ok with a warning.
    let tmp = tempfile::TempDir::new().unwrap();
    let result = verify_review_skill_available(tmp.path());
    assert!(
        result.is_err(),
        "missing skill must be a hard error, not a warning"
    );
}
