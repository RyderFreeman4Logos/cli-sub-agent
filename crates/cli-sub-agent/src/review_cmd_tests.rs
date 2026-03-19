use super::*;
use crate::cli::{Cli, Commands, ReviewMode, validate_review_args};
use clap::{Parser, error::ErrorKind};
use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig};
use csa_todo::{CriterionKind, CriterionStatus, SpecCriterion, SpecDocument, TodoManager};
use std::collections::HashMap;
use std::sync::LazyLock;
use tempfile::tempdir;

static REVIEW_ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

struct ScopedEnvVarRestore {
    key: &'static str,
    original: Option<String>,
}

impl ScopedEnvVarRestore {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for ScopedEnvVarRestore {
    fn drop(&mut self) {
        // SAFETY: restoration of test-scoped env mutation.
        unsafe {
            match self.original.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn project_config_with_enabled_tools(tools: &[&str]) -> ProjectConfig {
    let mut tool_map = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        tool_map.insert(
            tool.as_str().to_string(),
            ToolConfig {
                enabled: false,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
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
                ..Default::default()
            },
        );
    }

    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: tool_map,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        vcs: Default::default(),
    }
}

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

fn parse_review_error(argv: &[&str]) -> clap::Error {
    match Cli::try_parse_from(argv) {
        Ok(_) => panic!("review CLI args should fail to parse"),
        Err(err) => err,
    }
}

fn parse_or_validate_review_error(argv: &[&str]) -> clap::Error {
    match Cli::try_parse_from(argv) {
        Ok(cli) => match cli.command {
            Commands::Review(args) => {
                validate_review_args(&args).expect_err("review CLI args should fail validation")
            }
            _ => panic!("expected review subcommand"),
        },
        Err(err) => err,
    }
}

fn sample_spec_document(plan_ulid: &str, criterion_id: &str) -> SpecDocument {
    SpecDocument {
        schema_version: 1,
        plan_ulid: plan_ulid.to_string(),
        summary: format!("Spec summary for {plan_ulid}"),
        criteria: vec![SpecCriterion {
            kind: CriterionKind::Scenario,
            id: criterion_id.to_string(),
            description: format!("Criterion {criterion_id} must be satisfied."),
            status: CriterionStatus::Pending,
        }],
    }
}

// --- resolve_review_tool tests ---

#[test]
fn resolve_review_tool_prefers_cli_override() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["gemini-cli", "codex"]);
    let tool = resolve_review_tool(
        Some(ToolName::Codex),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
    )
    .unwrap();
    assert!(matches!(tool.0, ToolName::Codex));
}

#[test]
fn resolve_review_tool_global_auto_prefers_first_heterogeneous_tool() {
    let global = GlobalConfig::default();
    // Parent=claude-code (Anthropic family), so first heterogeneous candidate
    // in default order is gemini-cli.
    let cfg = project_config_with_enabled_tools(&["gemini-cli", "codex"]);
    let tool = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
    )
    .unwrap();
    assert!(matches!(tool.0, ToolName::GeminiCli));
}

#[test]
fn resolve_review_tool_global_auto_succeeds_with_single_heterogeneous_tool() {
    let global = GlobalConfig::default();
    // Only gemini-cli enabled — auto-selection should still succeed.
    let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
    let tool = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
    )
    .unwrap();
    assert!(matches!(tool.0, ToolName::GeminiCli));
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
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
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
    global.review.tool = csa_config::ToolSelection::Single("invalid-tool".to_string());
    let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
    let err = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        Some("codex"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
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
        tool: csa_config::ToolSelection::Single("opencode".to_string()),
        ..Default::default()
    });
    cfg.debate = None;

    let tool = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
    )
    .unwrap();
    assert!(matches!(tool.0, ToolName::Opencode));
}

#[test]
fn resolve_review_tool_project_auto_maps_to_heterogeneous_counterpart() {
    let global = GlobalConfig::default();
    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code"]);
    cfg.review = Some(csa_config::global::ReviewConfig {
        tool: csa_config::ToolSelection::Single("auto".to_string()),
        ..Default::default()
    });
    cfg.debate = None;

    let tool = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
    )
    .unwrap();
    assert!(matches!(tool.0, ToolName::Codex));
}

#[test]
fn resolve_review_tool_project_auto_prefers_priority_over_counterpart() {
    let mut global = GlobalConfig::default();
    global.preferences.tool_priority = vec!["opencode".to_string(), "claude-code".to_string()];

    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
    cfg.review = Some(csa_config::global::ReviewConfig {
        tool: csa_config::ToolSelection::Single("auto".to_string()),
        ..Default::default()
    });
    cfg.debate = None;

    let tool = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        Some("codex"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
    )
    .unwrap();
    assert!(matches!(tool.0, ToolName::Opencode));
}

#[test]
fn resolve_review_tool_unknown_priority_still_uses_auto_heterogeneous_selection() {
    let mut global = GlobalConfig::default();
    global.preferences.tool_priority = vec!["codexx".to_string()];

    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
    cfg.review = Some(csa_config::global::ReviewConfig {
        tool: csa_config::ToolSelection::Single("auto".to_string()),
        ..Default::default()
    });
    cfg.debate = None;

    let tool = resolve_review_tool(
        None,
        Some(&cfg),
        &global,
        Some("codex"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
    )
    .unwrap();
    assert!(matches!(tool.0, ToolName::Opencode));
}

// --- derive_scope tests ---

#[test]
fn derive_scope_uncommitted() {
    let args = ReviewArgs {
        tool: None,
        sa_mode: None,
        session: None,
        model: None,
        diff: true,
        branch: None,
        commit: None,
        range: None,
        files: None,
        fix: false,
        max_rounds: 3,
        review_mode: None,
        red_team: false,
        security_mode: "auto".to_string(),
        context: None,
        reviewers: 1,
        consensus: "majority".to_string(),
        cd: None,
        timeout: None,
        idle_timeout: None,
        initial_response_timeout: None,
        stream_stdout: false,
        no_stream_stdout: false,
        allow_fallback: false,
        force_override_user_config: false,
        spec: None,
        tier: None,
        force_ignore_tier_setting: false,
    };
    assert_eq!(derive_scope(&args), "uncommitted");
}

#[test]
fn derive_scope_commit() {
    let args = ReviewArgs {
        tool: None,
        sa_mode: None,
        session: None,
        model: None,
        diff: false,
        branch: None,
        commit: Some("abc123".to_string()),
        range: None,
        files: None,
        fix: false,
        max_rounds: 3,
        review_mode: None,
        red_team: false,
        security_mode: "auto".to_string(),
        context: None,
        reviewers: 1,
        consensus: "majority".to_string(),
        cd: None,
        timeout: None,
        idle_timeout: None,
        initial_response_timeout: None,
        stream_stdout: false,
        no_stream_stdout: false,
        allow_fallback: false,
        force_override_user_config: false,
        spec: None,
        tier: None,
        force_ignore_tier_setting: false,
    };
    assert_eq!(derive_scope(&args), "commit:abc123");
}

#[test]
fn derive_scope_range() {
    let args = ReviewArgs {
        tool: None,
        sa_mode: None,
        session: None,
        model: None,
        diff: false,
        branch: None,
        commit: None,
        range: Some("main...HEAD".to_string()),
        files: None,
        fix: false,
        max_rounds: 3,
        review_mode: None,
        red_team: false,
        security_mode: "auto".to_string(),
        context: None,
        reviewers: 1,
        consensus: "majority".to_string(),
        cd: None,
        timeout: None,
        idle_timeout: None,
        initial_response_timeout: None,
        stream_stdout: false,
        no_stream_stdout: false,
        allow_fallback: false,
        force_override_user_config: false,
        spec: None,
        tier: None,
        force_ignore_tier_setting: false,
    };
    assert_eq!(derive_scope(&args), "range:main...HEAD");
}

#[test]
fn derive_scope_files() {
    let args = ReviewArgs {
        tool: None,
        sa_mode: None,
        session: None,
        model: None,
        diff: false,
        branch: None,
        commit: None,
        range: None,
        files: Some("src/**/*.rs".to_string()),
        fix: false,
        max_rounds: 3,
        review_mode: None,
        red_team: false,
        security_mode: "auto".to_string(),
        context: None,
        reviewers: 1,
        consensus: "majority".to_string(),
        cd: None,
        timeout: None,
        idle_timeout: None,
        initial_response_timeout: None,
        stream_stdout: false,
        no_stream_stdout: false,
        allow_fallback: false,
        force_override_user_config: false,
        spec: None,
        tier: None,
        force_ignore_tier_setting: false,
    };
    assert_eq!(derive_scope(&args), "files:src/**/*.rs");
}

#[test]
fn derive_scope_default_branch() {
    let args = ReviewArgs {
        tool: None,
        sa_mode: None,
        session: None,
        model: None,
        diff: false,
        branch: Some("develop".to_string()),
        commit: None,
        range: None,
        files: None,
        fix: false,
        max_rounds: 3,
        review_mode: None,
        red_team: false,
        security_mode: "auto".to_string(),
        context: None,
        reviewers: 1,
        consensus: "majority".to_string(),
        cd: None,
        timeout: None,
        idle_timeout: None,
        initial_response_timeout: None,
        stream_stdout: false,
        no_stream_stdout: false,
        allow_fallback: false,
        force_override_user_config: false,
        spec: None,
        tier: None,
        force_ignore_tier_setting: false,
    };
    assert_eq!(derive_scope(&args), "base:develop");
}

#[test]
fn review_scope_allows_auto_discovery_for_default_branch_review() {
    let args = parse_review_args(&["csa", "review"]);
    assert!(review_scope_allows_auto_discovery(&args));
}

#[test]
fn review_scope_allows_auto_discovery_for_explicit_branch_review() {
    let args = parse_review_args(&["csa", "review", "--branch", "develop"]);
    assert!(review_scope_allows_auto_discovery(&args));
}

#[test]
fn review_scope_allows_auto_discovery_for_range_review() {
    let args = parse_review_args(&["csa", "review", "--range", "main...HEAD"]);
    assert!(review_scope_allows_auto_discovery(&args));
}

#[test]
fn review_scope_disables_auto_discovery_for_diff_review() {
    let args = parse_review_args(&["csa", "review", "--diff"]);
    assert!(!review_scope_allows_auto_discovery(&args));
}

#[test]
fn review_scope_disables_auto_discovery_for_commit_review() {
    let args = parse_review_args(&["csa", "review", "--commit", "abc123"]);
    assert!(!review_scope_allows_auto_discovery(&args));
}

#[test]
fn review_scope_disables_auto_discovery_for_files_review() {
    let args = parse_review_args(&["csa", "review", "--files", "src/**/*.rs"]);
    assert!(!review_scope_allows_auto_discovery(&args));
}

#[test]
fn review_cli_rejects_commit_with_range() {
    let err = parse_review_error(&["csa", "review", "--commit", "abc", "--range", "v1...v2"]);
    assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
}

#[test]
fn review_cli_rejects_diff_with_commit() {
    let err = parse_review_error(&["csa", "review", "--diff", "--commit", "abc"]);
    assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
}

#[test]
fn review_cli_rejects_diff_with_range() {
    let err = parse_review_error(&["csa", "review", "--diff", "--range", "main...HEAD"]);
    assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
}

#[test]
fn review_cli_rejects_files_with_diff() {
    let err = parse_review_error(&["csa", "review", "--files", "src/", "--diff"]);
    assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
}

#[test]
fn review_cli_rejects_branch_with_range() {
    let err = parse_review_error(&[
        "csa",
        "review",
        "--branch",
        "develop",
        "--range",
        "main...HEAD",
    ]);
    assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
}

#[test]
fn review_cli_rejects_branch_with_diff() {
    let err = parse_review_error(&["csa", "review", "--branch", "develop", "--diff"]);
    assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
}

#[test]
fn review_cli_accepts_single_scope_flags() {
    let diff = parse_review_args(&["csa", "review", "--diff"]);
    assert_eq!(derive_scope(&diff), "uncommitted");

    let commit = parse_review_args(&["csa", "review", "--commit", "abc123"]);
    assert_eq!(derive_scope(&commit), "commit:abc123");

    let range = parse_review_args(&["csa", "review", "--range", "main...HEAD"]);
    assert_eq!(derive_scope(&range), "range:main...HEAD");

    let files = parse_review_args(&["csa", "review", "--files", "src/"]);
    assert_eq!(derive_scope(&files), "files:src/");

    let branch = parse_review_args(&["csa", "review", "--branch", "develop"]);
    assert_eq!(derive_scope(&branch), "base:develop");
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
    assert!(!args.allow_fallback);
}

#[test]
fn review_cli_parses_allow_fallback_flag() {
    let args = parse_review_args(&["csa", "review", "--diff", "--allow-fallback"]);
    assert!(args.allow_fallback);
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
    // Default should always be BufferOnly unless explicitly overridden.
    let mode = resolve_review_stream_mode(false, false);
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

#[test]
fn sanitize_review_output_prefers_summary_and_details_sections() {
    let raw = "noise line\n\
<!-- CSA:SECTION:summary -->\nSummary body\n<!-- CSA:SECTION:summary:END -->\n\
<!-- CSA:SECTION:details -->\nDetails body\n<!-- CSA:SECTION:details:END -->\n\
trailing noise";
    let sanitized = sanitize_review_output(raw);
    assert!(sanitized.contains("CSA:SECTION:summary"));
    assert!(sanitized.contains("Summary body"));
    assert!(sanitized.contains("CSA:SECTION:details"));
    assert!(sanitized.contains("Details body"));
    assert!(!sanitized.contains("noise line"));
    assert!(!sanitized.contains("trailing noise"));
}

#[test]
fn sanitize_review_output_falls_back_when_sections_missing() {
    let raw = "plain output without markers";
    let sanitized = sanitize_review_output(raw);
    assert_eq!(sanitized, raw);
}

#[path = "review_cmd_tier_tests.rs"]
mod tier_tests;

#[path = "review_cmd_tests_tail.rs"]
mod tail_tests;
