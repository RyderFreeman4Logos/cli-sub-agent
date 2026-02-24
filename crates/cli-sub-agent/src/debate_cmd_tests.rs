use super::*;
use csa_config::global::ReviewConfig;
use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig};
use serde_json::Value;
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
    let (tool, mode) = resolve_debate_tool(
        Some(ToolName::Codex),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
    )
    .unwrap();
    assert!(matches!(tool, ToolName::Codex));
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn resolve_debate_tool_auto_maps_heterogeneous() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["codex"]);
    let (tool, mode) = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
    )
    .unwrap();
    assert!(matches!(tool, ToolName::Codex));
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn resolve_debate_tool_auto_maps_reverse() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["claude-code"]);
    let (tool, mode) = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("codex"),
        std::path::Path::new("/tmp/test-project"),
        false,
    )
    .unwrap();
    assert!(matches!(tool, ToolName::ClaudeCode));
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn resolve_debate_tool_same_model_fallback_when_no_parent() {
    // With same_model_fallback enabled (default), no parent context should fall
    // back to same-model adversarial using any available tool.
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["opencode"]);
    let (tool, mode) = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        None,
        std::path::Path::new("/tmp/test-project"),
        false,
    )
    .unwrap();
    // Falls back to first known tool (same-model adversarial)
    assert_eq!(mode, DebateMode::SameModelAdversarial);
    // Tool is from all_known_tools since no parent was detected
    assert!(matches!(
        tool,
        ToolName::GeminiCli | ToolName::Opencode | ToolName::Codex | ToolName::ClaudeCode
    ));
}

#[test]
fn resolve_debate_tool_same_model_fallback_disabled_errors_without_parent() {
    let mut global = GlobalConfig::default();
    global.debate.same_model_fallback = false;
    let cfg = project_config_with_enabled_tools(&["opencode"]);
    let err = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        None,
        std::path::Path::new("/tmp/test-project"),
        false,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("AUTO debate tool selection failed")
    );
}

#[test]
fn resolve_debate_tool_same_model_fallback_uses_parent_tool() {
    // When only the parent tool family is available, fallback uses the parent tool.
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["opencode"]);
    let (tool, mode) = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("opencode"),
        std::path::Path::new("/tmp/test-project"),
        false,
    )
    .unwrap();
    assert!(matches!(tool, ToolName::Opencode));
    assert_eq!(mode, DebateMode::SameModelAdversarial);
}

#[test]
fn resolve_debate_tool_same_model_fallback_disabled_errors_on_unknown_parent() {
    let mut global = GlobalConfig::default();
    global.debate.same_model_fallback = false;
    let cfg = project_config_with_enabled_tools(&["opencode"]);
    let err = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("opencode"),
        std::path::Path::new("/tmp/test-project"),
        false,
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
        ..Default::default()
    });

    let (tool, mode) = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
    )
    .unwrap();
    assert!(matches!(tool, ToolName::Opencode));
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn resolve_debate_tool_project_auto_maps_heterogeneous() {
    let global = GlobalConfig::default();
    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code"]);
    cfg.debate = Some(ReviewConfig {
        tool: "auto".to_string(),
        ..Default::default()
    });

    let (tool, mode) = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
    )
    .unwrap();
    assert!(matches!(tool, ToolName::Codex));
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn resolve_debate_tool_project_auto_prefers_priority_over_counterpart() {
    let mut global = GlobalConfig::default();
    global.preferences.tool_priority = vec!["opencode".to_string(), "claude-code".to_string()];

    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
    cfg.debate = Some(ReviewConfig {
        tool: "auto".to_string(),
        ..Default::default()
    });

    let (tool, mode) = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("codex"),
        std::path::Path::new("/tmp/test-project"),
        false,
    )
    .unwrap();
    assert!(matches!(tool, ToolName::Opencode));
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn resolve_debate_tool_ignores_unknown_priority_entries() {
    let mut global = GlobalConfig::default();
    global.preferences.tool_priority = vec!["codexx".to_string()];

    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
    cfg.debate = Some(ReviewConfig {
        tool: "auto".to_string(),
        ..Default::default()
    });

    let (tool, mode) = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("codex"),
        std::path::Path::new("/tmp/test-project"),
        false,
    )
    .unwrap();
    assert!(matches!(tool, ToolName::ClaudeCode));
    assert_eq!(mode, DebateMode::Heterogeneous);
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

#[test]
fn extract_verdict_prefers_explicit_verdict_line() {
    let output = r#"
Debate notes...
Final verdict: APPROVE
Confidence: high
"#;
    assert_eq!(extract_verdict(output), "APPROVE");
}

#[test]
fn extract_verdict_defaults_to_revise_when_missing() {
    let output = "No explicit verdict included.";
    assert_eq!(extract_verdict(output), "REVISE");
}

#[test]
fn extract_confidence_detects_medium() {
    let output = "Confidence: medium";
    assert_eq!(extract_confidence(output), "medium");
}

#[test]
fn extract_one_line_summary_prefers_summary_prefix() {
    let output = r#"
# Debate
Summary: Adopt bounded retries and idempotency keys.
- Key point A
"#;
    let summary = extract_one_line_summary(output, "fallback");
    assert_eq!(summary, "Adopt bounded retries and idempotency keys.");
}

#[test]
fn extract_key_points_reads_bullets_and_numbers() {
    let output = r#"
- First key point
1. Second key point
2) Third key point
"#;
    let points = extract_key_points(output, "fallback");
    assert_eq!(
        points,
        vec!["First key point", "Second key point", "Third key point"]
    );
}

#[test]
fn format_debate_stdout_summary_contains_required_fields() {
    let summary = DebateSummary {
        verdict: "APPROVE".to_string(),
        confidence: "high".to_string(),
        summary: "Proceed with the proposal.".to_string(),
        key_points: vec!["Point".to_string()],
        mode: DebateMode::Heterogeneous,
    };
    let line = format_debate_stdout_summary(&summary);
    assert!(line.contains("APPROVE"));
    assert!(line.contains("high"));
    assert!(line.contains("Proceed with the proposal."));
    assert!(!line.contains("DEGRADED"));
}

#[test]
fn format_debate_stdout_summary_shows_degradation_for_same_model() {
    let summary = DebateSummary {
        verdict: "REVISE".to_string(),
        confidence: "medium".to_string(),
        summary: "Need more evidence.".to_string(),
        key_points: vec![],
        mode: DebateMode::SameModelAdversarial,
    };
    let line = format_debate_stdout_summary(&summary);
    assert!(line.contains("DEGRADED"));
    assert!(line.contains("same-model adversarial"));
}

#[test]
fn persist_debate_output_artifacts_writes_json_and_markdown() {
    let tmp = tempfile::TempDir::new().unwrap();
    let session_dir = tmp.path();
    std::fs::create_dir_all(session_dir.join("output")).unwrap();

    let summary = DebateSummary {
        verdict: "REVISE".to_string(),
        confidence: "low".to_string(),
        summary: "Need more data before rollout.".to_string(),
        key_points: vec!["Insufficient benchmark evidence.".to_string()],
        mode: DebateMode::Heterogeneous,
    };
    let transcript = "# Debate transcript\n\nFull content.";
    let artifacts = persist_debate_output_artifacts(session_dir, &summary, transcript).unwrap();

    assert_eq!(artifacts.len(), 2);
    assert_eq!(artifacts[0].path, "output/debate-verdict.json");
    assert_eq!(artifacts[1].path, "output/debate-transcript.md");

    let verdict_path = session_dir.join("output/debate-verdict.json");
    let verdict_json = std::fs::read_to_string(verdict_path).unwrap();
    let parsed: Value = serde_json::from_str(&verdict_json).unwrap();
    assert_eq!(parsed["verdict"], "REVISE");
    assert_eq!(parsed["confidence"], "low");
    assert_eq!(parsed["summary"], "Need more data before rollout.");
    assert_eq!(parsed["key_points"][0], "Insufficient benchmark evidence.");
    assert!(parsed["timestamp"].as_str().is_some());
    // Heterogeneous mode should not include mode annotation
    assert!(parsed.get("mode").is_none());

    let transcript_path = session_dir.join("output/debate-transcript.md");
    let transcript_content = std::fs::read_to_string(transcript_path).unwrap();
    assert_eq!(transcript_content, transcript);
}

#[test]
fn persist_debate_output_artifacts_includes_mode_for_same_model() {
    let tmp = tempfile::TempDir::new().unwrap();
    let session_dir = tmp.path();
    std::fs::create_dir_all(session_dir.join("output")).unwrap();

    let summary = DebateSummary {
        verdict: "APPROVE".to_string(),
        confidence: "medium".to_string(),
        summary: "Acceptable with caveats.".to_string(),
        key_points: vec![],
        mode: DebateMode::SameModelAdversarial,
    };
    let artifacts = persist_debate_output_artifacts(session_dir, &summary, "transcript").unwrap();

    let verdict_path = session_dir.join("output/debate-verdict.json");
    let verdict_json = std::fs::read_to_string(verdict_path).unwrap();
    let parsed: Value = serde_json::from_str(&verdict_json).unwrap();
    assert_eq!(parsed["mode"], "same-model adversarial, not heterogeneous");
    assert_eq!(artifacts.len(), 2);
}

#[test]
fn extract_debate_summary_does_not_leak_provider_session_id() {
    // Simulate sanitized output where provider ID has already been replaced by
    // render_debate_output before reaching extract_debate_summary.
    let provider_id = "provider-secret-id-abc123";
    let meta_id = "01KHMETA0000000000000000";
    let raw_output = format!(
        "session_id={provider_id}\nSummary: The plan looks solid.\nVerdict: APPROVE\nConfidence: high\n- Good architecture\n"
    );
    // Simulate render_debate_output sanitization
    let sanitized = raw_output.replace(provider_id, meta_id);
    let summary = extract_debate_summary(&sanitized, "fallback", DebateMode::Heterogeneous);

    assert!(
        !summary.summary.contains(provider_id),
        "summary must not contain provider id"
    );
    assert!(!summary.verdict.contains(provider_id));
    for point in &summary.key_points {
        assert!(
            !point.contains(provider_id),
            "key_point must not contain provider id"
        );
    }
    // Verify meta_id is present (or harmless if not matched by extraction heuristics)
    assert_eq!(summary.verdict, "APPROVE");
    assert_eq!(summary.confidence, "high");
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
fn debate_cli_parses_thinking_flag() {
    let args = parse_debate_args(&["csa", "debate", "--thinking", "high", "question"]);
    assert_eq!(args.thinking.as_deref(), Some("high"));
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
    assert_eq!(args.thinking, None);
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

// --- resolve_debate_thinking tests ---

#[test]
fn resolve_debate_thinking_prefers_cli_over_config() {
    let thinking = resolve_debate_thinking(Some("low"), Some("high"));
    assert_eq!(thinking.as_deref(), Some("low"));
}

#[test]
fn resolve_debate_thinking_uses_config_when_cli_missing() {
    let thinking = resolve_debate_thinking(None, Some("medium"));
    assert_eq!(thinking.as_deref(), Some("medium"));
}

#[test]
fn resolve_debate_thinking_defaults_none_for_backward_compatibility() {
    let thinking = resolve_debate_thinking(None, None);
    assert_eq!(thinking, None);
}

#[test]
fn resolve_debate_timeout_prefers_cli_over_global() {
    let timeout = resolve_debate_timeout_seconds(Some(120), Some(600));
    assert_eq!(timeout, Some(120));
}

#[test]
fn resolve_debate_timeout_uses_global_then_none() {
    assert_eq!(resolve_debate_timeout_seconds(None, Some(600)), Some(600));
    assert_eq!(resolve_debate_timeout_seconds(None, None), None);
}

#[test]
fn wall_clock_timeout_guard_allows_within_budget() {
    let start = tokio::time::Instant::now();
    assert!(ensure_debate_wall_clock_within_timeout(start, Some(1)).is_ok());
}

#[test]
fn wall_clock_timeout_guard_rejects_elapsed_budget() {
    let start = tokio::time::Instant::now() - std::time::Duration::from_secs(2);
    let err = ensure_debate_wall_clock_within_timeout(start, Some(1)).unwrap_err();
    assert!(err.to_string().contains("Wall-clock timeout exceeded (1s)"));
}

#[test]
fn retry_policy_only_retries_transient_once() {
    use crate::debate_errors::DebateErrorKind;

    assert!(should_retry_debate_after_error(
        &DebateErrorKind::Transient("oom".to_string()),
        0
    ));
    assert!(!should_retry_debate_after_error(
        &DebateErrorKind::Transient("oom".to_string()),
        1
    ));
    assert!(!should_retry_debate_after_error(
        &DebateErrorKind::Deterministic("arg".to_string()),
        0
    ));
}

#[test]
fn still_working_backoff_uses_five_seconds() {
    assert_eq!(STILL_WORKING_BACKOFF, std::time::Duration::from_secs(5));
}

#[tokio::test]
async fn still_working_backoff_waits_before_retry() {
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        wait_for_still_working_backoff(),
    )
    .await;
    assert!(
        result.is_err(),
        "StillWorking backoff should not complete immediately"
    );
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
