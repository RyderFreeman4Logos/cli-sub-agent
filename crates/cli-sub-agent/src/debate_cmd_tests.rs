use super::*;
use csa_config::global::ReviewConfig;
use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig};
use std::collections::HashMap;

fn project_config_with_enabled_tools(tools: &[&str]) -> ProjectConfig {
    let mut tool_map = HashMap::new();
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
        preferences: None,
        session: Default::default(),
    }
}

#[test]
fn resolve_debate_tool_prefers_cli_override() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
    let tool = resolve_debate_tool(
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
fn resolve_debate_tool_auto_maps_heterogeneous() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["codex"]);
    let tool = resolve_debate_tool(
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
fn resolve_debate_tool_auto_maps_reverse() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["claude-code"]);
    let tool = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("codex"),
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap();
    assert!(matches!(tool, ToolName::ClaudeCode));
}

#[test]
fn resolve_debate_tool_errors_without_parent_context() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["opencode"]);
    let err = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        None,
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("AUTO debate tool selection failed")
    );
}

#[test]
fn resolve_debate_tool_errors_on_unknown_parent() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["opencode"]);
    let err = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("opencode"),
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("AUTO debate tool selection failed")
    );
}

#[test]
fn resolve_debate_tool_prefers_project_override() {
    let global = GlobalConfig::default();
    let mut cfg = project_config_with_enabled_tools(&["codex", "opencode"]);
    cfg.debate = Some(ReviewConfig {
        tool: "opencode".to_string(),
    });

    let tool = resolve_debate_tool(
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
fn resolve_debate_tool_project_auto_maps_heterogeneous() {
    let global = GlobalConfig::default();
    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code"]);
    cfg.debate = Some(ReviewConfig {
        tool: "auto".to_string(),
    });

    let tool = resolve_debate_tool(
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
fn resolve_debate_tool_project_auto_prefers_priority_over_counterpart() {
    let mut global = GlobalConfig::default();
    global.preferences.tool_priority = vec!["opencode".to_string(), "claude-code".to_string()];

    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
    cfg.debate = Some(ReviewConfig {
        tool: "auto".to_string(),
    });

    let tool = resolve_debate_tool(
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
fn resolve_debate_tool_ignores_unknown_priority_entries() {
    let mut global = GlobalConfig::default();
    global.preferences.tool_priority = vec!["codexx".to_string()];

    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
    cfg.debate = Some(ReviewConfig {
        tool: "auto".to_string(),
    });

    let tool = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("codex"),
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap();
    assert!(matches!(tool, ToolName::ClaudeCode));
}

#[test]
fn build_debate_instruction_new_debate() {
    let prompt = build_debate_instruction("Should we use gRPC or REST?", false, 3);
    assert!(prompt.contains("debate skill"));
    assert!(prompt.contains("Should we use gRPC or REST?"));
    assert!(!prompt.contains("continuation=true"));
    assert!(prompt.contains("rounds=3"));
}

#[test]
fn build_debate_instruction_continuation() {
    let prompt = build_debate_instruction("I disagree because X", true, 3);
    assert!(prompt.contains("debate skill"));
    assert!(prompt.contains("continuation=true"));
    assert!(prompt.contains("I disagree because X"));
    assert!(prompt.contains("rounds=3"));
}

#[test]
fn build_debate_instruction_custom_rounds() {
    let prompt = build_debate_instruction("topic", false, 5);
    assert!(prompt.contains("rounds=5"));
}

#[test]
fn render_debate_output_appends_meta_session_id() {
    let output = render_debate_output("debate answer", "01ARZ3NDEKTSV4RRFFQ69G5FAV", None);
    assert!(output.contains("debate answer"));
    assert!(output.contains("CSA Meta Session ID: 01ARZ3NDEKTSV4RRFFQ69G5FAV"));
}

#[test]
fn render_debate_output_replaces_provider_id_with_meta_id() {
    let provider = "019c5589-3c84-7f03-b9c4-9f0a164c4eb2";
    let meta = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let tool_output = format!("session_id={provider}\nresult=ok");

    let output = render_debate_output(&tool_output, meta, Some(provider));
    assert!(!output.contains(provider));
    assert!(output.contains(meta));
}

// --- CLI parse tests for timeout/stream flags (#146) ---

fn parse_debate_args(argv: &[&str]) -> crate::cli::DebateArgs {
    use crate::cli::{Cli, Commands};
    use clap::Parser;
    let cli = Cli::try_parse_from(argv).expect("debate CLI args should parse");
    match cli.command {
        Commands::Debate(args) => args,
        _ => panic!("expected debate subcommand"),
    }
}

#[test]
fn debate_cli_parses_timeout_flag() {
    let args = parse_debate_args(&["csa", "debate", "--timeout", "120", "question"]);
    assert_eq!(args.timeout, Some(120));
}

#[test]
fn debate_cli_parses_idle_timeout_flag() {
    let args = parse_debate_args(&["csa", "debate", "--idle-timeout", "60", "question"]);
    assert_eq!(args.idle_timeout, Some(60));
}

#[test]
fn debate_cli_parses_both_timeouts() {
    let args = parse_debate_args(&[
        "csa",
        "debate",
        "--timeout",
        "300",
        "--idle-timeout",
        "30",
        "question",
    ]);
    assert_eq!(args.timeout, Some(300));
    assert_eq!(args.idle_timeout, Some(30));
}

#[test]
fn debate_cli_parses_stream_stdout_flag() {
    let args = parse_debate_args(&["csa", "debate", "--stream-stdout", "question"]);
    assert!(args.stream_stdout);
    assert!(!args.no_stream_stdout);
}

#[test]
fn debate_cli_parses_no_stream_stdout_flag() {
    let args = parse_debate_args(&["csa", "debate", "--no-stream-stdout", "question"]);
    assert!(!args.stream_stdout);
    assert!(args.no_stream_stdout);
}

#[test]
fn debate_cli_defaults_no_timeout() {
    let args = parse_debate_args(&["csa", "debate", "question"]);
    assert_eq!(args.timeout, None);
    assert_eq!(args.idle_timeout, None);
    assert!(!args.stream_stdout);
    assert!(!args.no_stream_stdout);
}

#[test]
fn debate_cli_rejects_zero_timeout() {
    use clap::Parser;
    let result = crate::cli::Cli::try_parse_from(["csa", "debate", "--timeout", "0", "question"]);
    assert!(result.is_err(), "timeout=0 should be rejected");
}

#[test]
fn debate_cli_rejects_zero_idle_timeout() {
    use clap::Parser;
    let result =
        crate::cli::Cli::try_parse_from(["csa", "debate", "--idle-timeout", "0", "question"]);
    assert!(result.is_err(), "idle_timeout=0 should be rejected");
}

// --- CLI parse tests for --rounds flag (#138) ---

#[test]
fn debate_cli_parses_rounds_flag() {
    let args = parse_debate_args(&["csa", "debate", "--rounds", "5", "question"]);
    assert_eq!(args.rounds, 5);
}

#[test]
fn debate_cli_rounds_defaults_to_3() {
    let args = parse_debate_args(&["csa", "debate", "question"]);
    assert_eq!(args.rounds, 3);
}

#[test]
fn debate_cli_rejects_zero_rounds() {
    use clap::Parser;
    let result = crate::cli::Cli::try_parse_from(["csa", "debate", "--rounds", "0", "question"]);
    assert!(result.is_err(), "rounds=0 should be rejected");
}

// --- resolve_debate_stream_mode tests ---

#[test]
fn debate_stream_mode_default_non_tty_is_buffer_only() {
    // In test environment (non-TTY stderr), default should be BufferOnly.
    // Note: in interactive TTY, default would be TeeToStderr (symmetric with review, #139)
    let mode = resolve_debate_stream_mode(false, false);
    assert!(matches!(mode, csa_process::StreamMode::BufferOnly));
}

#[test]
fn debate_stream_mode_explicit_stream() {
    let mode = resolve_debate_stream_mode(true, false);
    assert!(matches!(mode, csa_process::StreamMode::TeeToStderr));
}

#[test]
fn debate_stream_mode_explicit_no_stream() {
    let mode = resolve_debate_stream_mode(false, true);
    assert!(matches!(mode, csa_process::StreamMode::BufferOnly));
}

// --- verify_debate_skill_available tests (#140) ---

#[test]
fn verify_debate_skill_missing_returns_actionable_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let err = verify_debate_skill_available(tmp.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Debate pattern not found"),
        "should mention missing pattern: {msg}"
    );
    assert!(
        msg.contains("csa skill install"),
        "should include install guidance: {msg}"
    );
    assert!(
        msg.contains("patterns/debate"),
        "should list searched paths: {msg}"
    );
}

#[test]
fn verify_debate_skill_present_succeeds() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Pattern layout: .csa/patterns/debate/skills/debate/SKILL.md
    let skill_dir = tmp
        .path()
        .join(".csa")
        .join("patterns")
        .join("debate")
        .join("skills")
        .join("debate");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "# Debate Skill\nStructured debate.",
    )
    .unwrap();

    assert!(verify_debate_skill_available(tmp.path()).is_ok());
}

#[test]
fn verify_debate_skill_no_fallback_without_skill() {
    // Ensure no execution path silently downgrades when skill is missing.
    // The verify function must return Err — it must NOT return Ok with a warning.
    let tmp = tempfile::TempDir::new().unwrap();
    let result = verify_debate_skill_available(tmp.path());
    assert!(
        result.is_err(),
        "missing skill must be a hard error, not a warning"
    );
}
