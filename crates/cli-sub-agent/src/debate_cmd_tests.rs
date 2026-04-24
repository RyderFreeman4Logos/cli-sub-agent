use super::*;
use crate::debate_cmd_output::*;
use crate::debate_cmd_resolve::resolve_debate_tool;
use crate::test_env_lock::TEST_ENV_LOCK;
use csa_config::global::ReviewConfig;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig};
use csa_core::types::ToolName;
use csa_session::{SessionArtifact, create_session, load_result, save_result};
use serde_json::Value;
use std::collections::HashMap;

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe {
            match self.original.as_deref() {
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
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

#[test]
fn resolve_debate_tool_prefers_cli_override() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["gemini-cli", "codex"]);
    let (tool, mode, _) = resolve_debate_tool(
        Some(ToolName::Codex),
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
    assert!(matches!(tool, ToolName::Codex));
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn resolve_debate_tool_auto_maps_heterogeneous() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let _available_guard =
        EnvVarGuard::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["codex"]);
    let (tool, mode, _) = resolve_debate_tool(
        None,
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
    assert!(matches!(tool, ToolName::Codex));
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn resolve_debate_tool_auto_maps_reverse() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let _available_guard =
        EnvVarGuard::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["claude-code"]);
    let (tool, mode, _) = resolve_debate_tool(
        None,
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
    assert!(matches!(tool, ToolName::ClaudeCode));
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn resolve_debate_tool_same_model_fallback_when_no_parent() {
    // With same_model_fallback enabled (default), no parent context should fall
    // back to same-model adversarial using any explicitly available tool.
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let _available_guard =
        EnvVarGuard::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["opencode"]);
    let (tool, mode, _) = resolve_debate_tool(
        None,
        None,
        Some(&cfg),
        &global,
        None,
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
    )
    .unwrap();
    assert_eq!(mode, DebateMode::SameModelAdversarial);
    assert!(matches!(tool, ToolName::Opencode));
}

#[test]
fn resolve_debate_tool_same_model_fallback_disabled_errors_without_parent() {
    let mut global = GlobalConfig::default();
    global.debate.same_model_fallback = false;
    let cfg = project_config_with_enabled_tools(&["opencode"]);
    let err = resolve_debate_tool(
        None,
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
            .contains("AUTO debate tool selection failed")
    );
}

#[test]
fn resolve_debate_tool_same_model_fallback_uses_parent_tool() {
    // When only the parent tool family is available, fallback uses the parent tool.
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let _available_guard =
        EnvVarGuard::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["opencode"]);
    let (tool, mode, _) = resolve_debate_tool(
        None,
        None,
        Some(&cfg),
        &global,
        Some("opencode"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
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
        None,
        Some(&cfg),
        &global,
        Some("opencode"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
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
        tool: csa_config::ToolSelection::Single("opencode".to_string()),
        ..Default::default()
    });

    let (tool, mode, _) = resolve_debate_tool(
        None,
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
    assert!(matches!(tool, ToolName::Opencode));
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn resolve_debate_tool_project_auto_maps_heterogeneous() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let _available_guard =
        EnvVarGuard::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let global = GlobalConfig::default();
    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code"]);
    cfg.debate = Some(ReviewConfig {
        tool: csa_config::ToolSelection::Single("auto".to_string()),
        ..Default::default()
    });

    let (tool, mode, _) = resolve_debate_tool(
        None,
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
    assert!(matches!(tool, ToolName::Codex));
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn resolve_debate_tool_project_auto_prefers_priority_over_counterpart() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let _available_guard =
        EnvVarGuard::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let mut global = GlobalConfig::default();
    global.preferences.tool_priority = vec!["opencode".to_string(), "claude-code".to_string()];

    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
    cfg.debate = Some(ReviewConfig {
        tool: csa_config::ToolSelection::Single("auto".to_string()),
        ..Default::default()
    });

    let (tool, mode, _) = resolve_debate_tool(
        None,
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
    assert!(matches!(tool, ToolName::Opencode));
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn resolve_debate_tool_unknown_priority_still_uses_auto_heterogeneous_selection() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let _available_guard =
        EnvVarGuard::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let mut global = GlobalConfig::default();
    global.preferences.tool_priority = vec!["codexx".to_string()];

    let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
    cfg.debate = Some(ReviewConfig {
        tool: csa_config::ToolSelection::Single("auto".to_string()),
        ..Default::default()
    });

    let (tool, mode, _) = resolve_debate_tool(
        None,
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
    assert!(matches!(tool, ToolName::Opencode));
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn build_debate_instruction_contains_safety_preamble() {
    let prompt = build_debate_instruction("topic", false, 3);
    assert!(
        prompt.contains("INSIDE a CSA subprocess"),
        "Debate instruction must identify subprocess context"
    );
    assert!(
        prompt.contains("DEBATE SAFETY"),
        "Debate instruction must constrain to read-only operations"
    );
    assert!(
        !prompt.contains("Do NOT invoke"),
        "Legacy blanket anti-csa text must not be reintroduced (breaks fractal recursion contract)"
    );
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
        decision: None,
        confidence: "high".to_string(),
        summary: "Proceed with the proposal.".to_string(),
        key_points: vec!["Point".to_string()],
        failure_reason: None,
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
        decision: None,
        confidence: "medium".to_string(),
        summary: "Need more evidence.".to_string(),
        key_points: vec![],
        failure_reason: None,
        mode: DebateMode::SameModelAdversarial,
    };
    let line = format_debate_stdout_summary(&summary);
    assert!(line.contains("DEGRADED"));
    assert!(line.contains("same-model adversarial"));
}

#[test]
fn format_debate_stdout_text_includes_summary_and_transcript() {
    let summary = DebateSummary {
        verdict: "REVISE".to_string(),
        decision: None,
        confidence: "medium".to_string(),
        summary: "Needs stronger evidence.".to_string(),
        key_points: vec![],
        failure_reason: None,
        mode: DebateMode::Heterogeneous,
    };
    let transcript = "<!-- CSA:SECTION:summary -->\nDetailed transcript\n";
    let text = format_debate_stdout_text(&summary, transcript);

    assert!(text.starts_with("Debate verdict: REVISE"));
    assert!(text.contains("Needs stronger evidence."));
    assert!(text.contains("Detailed transcript"));
}

#[test]
fn extract_one_line_summary_ignores_csa_section_marker_lines() {
    let output = r#"
<!-- CSA:SECTION:summary -->
<!-- CSA:SECTION:summary:END -->
Summary: Keep full debate transcript in stdout.
"#;
    let summary = extract_one_line_summary(output, "fallback");
    assert_eq!(summary, "Keep full debate transcript in stdout.");
}

#[test]
fn persist_debate_output_artifacts_includes_mode_for_same_model() {
    let tmp = tempfile::TempDir::new().unwrap();
    let session_dir = tmp.path();
    std::fs::create_dir_all(session_dir.join("output")).unwrap();

    let summary = DebateSummary {
        verdict: "APPROVE".to_string(),
        decision: None,
        confidence: "medium".to_string(),
        summary: "Acceptable with caveats.".to_string(),
        key_points: vec![],
        failure_reason: None,
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
fn append_debate_artifacts_to_result_updates_summary_and_artifacts() {
    let temp = tempfile::TempDir::new().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", temp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);

    let project_root = temp.path();
    let session = create_session(project_root, Some("debate"), None, None).unwrap();

    save_result(
        project_root,
        &session.meta_session_id,
        &csa_session::SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "<!-- CSA:SECTION:summary:END -->".to_string(),
            tool: "codex".to_string(),
            started_at: chrono::Utc::now(),
            completed_at: chrono::Utc::now(),
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
            manager_fields: Default::default(),
        },
    )
    .unwrap();

    let debate_summary = DebateSummary {
        verdict: "APPROVE".to_string(),
        decision: None,
        confidence: "high".to_string(),
        summary: "Persist the parsed debate summary.".to_string(),
        key_points: vec!["Key point".to_string()],
        failure_reason: None,
        mode: DebateMode::Heterogeneous,
    };
    let artifacts = vec![SessionArtifact::new("output/debate-verdict.json")];

    append_debate_artifacts_to_result(
        project_root,
        &session.meta_session_id,
        &artifacts,
        &debate_summary,
    )
    .unwrap();

    let saved = load_result(project_root, &session.meta_session_id)
        .unwrap()
        .expect("saved result");
    assert_eq!(saved.summary, "Persist the parsed debate summary.");
    assert!(
        saved
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/debate-verdict.json")
    );
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
fn debate_cli_parses_global_json_format() {
    use crate::cli::{Cli, Commands};
    use clap::Parser;
    use csa_core::types::OutputFormat;

    let cli = Cli::try_parse_from(["csa", "--format", "json", "debate", "question"])
        .expect("cli should parse global json format for debate");

    assert!(matches!(cli.format, OutputFormat::Json));
    match cli.command {
        Commands::Debate(args) => assert_eq!(args.question.as_deref(), Some("question")),
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
        "250",
        "--idle-timeout",
        "30",
        "question",
    ]);
    assert_eq!(args.timeout, Some(250));
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
fn debate_cli_parses_hint_difficulty_flag() {
    let args = parse_debate_args(&[
        "csa",
        "debate",
        "--tool",
        "claude-code",
        "--hint-difficulty",
        "architecture_design",
        "question",
    ]);
    assert_eq!(args.hint_difficulty.as_deref(), Some("architecture_design"));
}

#[test]
fn debate_cli_hint_difficulty_conflicts_with_tier() {
    use clap::Parser;

    let err = match crate::cli::Cli::try_parse_from([
        "csa",
        "debate",
        "--hint-difficulty",
        "architecture_design",
        "--tier",
        "tier-2-standard",
        "question",
    ]) {
        Ok(_) => panic!("hint-difficulty and tier should conflict"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
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

#[path = "debate_cmd_tier_tests.rs"]
mod tier_tests;

#[path = "debate_cmd_execute_tier_tests.rs"]
mod execute_tier_tests;

#[path = "debate_cmd_tests_tail.rs"]
mod tests_tail;
