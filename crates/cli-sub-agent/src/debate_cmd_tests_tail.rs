use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use std::path::Path;
use tempfile::tempdir;

fn write_debate_project_config(project_root: &Path, config: &ProjectConfig) {
    let config_path = ProjectConfig::config_path(project_root);
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(config_path, toml::to_string_pretty(config).unwrap()).unwrap();
}

fn install_pattern(project_root: &Path, name: &str) {
    let skill_dir = project_root
        .join(".csa")
        .join("patterns")
        .join(name)
        .join("skills")
        .join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# test pattern\n").unwrap();
}

// --- resolve_debate_thinking tests ---

#[test]
fn resolve_debate_model_prefers_cli_over_config() {
    let model = resolve_debate_model(Some("gpt-5"), Some("gemini-2.5"), false);
    assert_eq!(model.as_deref(), Some("gpt-5"));
}

#[test]
fn resolve_debate_model_ignores_config_when_tier_active() {
    let model = resolve_debate_model(None, Some("gemini-2.5"), true);
    assert_eq!(model, None);
}

#[test]
fn resolve_debate_tool_prefers_model_spec_override() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["codex"]);
    let (tool, mode, model_spec) = resolve_debate_tool(
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
    assert!(matches!(tool, ToolName::Codex));
    assert_eq!(mode, DebateMode::Heterogeneous);
    assert_eq!(model_spec.as_deref(), Some("codex/openai/gpt-5.4/xhigh"));
}

#[test]
fn resolve_debate_tool_rejects_model_spec_with_tier() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["codex"]);
    let err = resolve_debate_tool(
        None,
        Some("codex/openai/gpt-5.4/xhigh"),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("tier-2-standard"),
        false,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("--model-spec and --tier are mutually exclusive"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn resolve_debate_thinking_prefers_cli_over_config() {
    let thinking = resolve_debate_thinking(Some("low"), Some("high"), false);
    assert_eq!(thinking.as_deref(), Some("low"));
}

#[test]
fn resolve_debate_thinking_uses_config_when_cli_missing() {
    let thinking = resolve_debate_thinking(None, Some("medium"), false);
    assert_eq!(thinking.as_deref(), Some("medium"));
}

#[test]
fn resolve_debate_thinking_defaults_none_for_backward_compatibility() {
    let thinking = resolve_debate_thinking(None, None, false);
    assert_eq!(thinking, None);
}

#[test]
fn resolve_debate_thinking_ignores_config_when_tier_active() {
    let thinking = resolve_debate_thinking(None, Some("medium"), true);
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
        0,
        false
    ));
    assert!(!should_retry_debate_after_error(
        &DebateErrorKind::Transient("oom".to_string()),
        1,
        false
    ));
    assert!(!should_retry_debate_after_error(
        &DebateErrorKind::Deterministic("arg".to_string()),
        0,
        false
    ));
}

#[test]
fn retry_policy_suppressed_when_no_failover() {
    use crate::debate_errors::DebateErrorKind;

    assert!(!should_retry_debate_after_error(
        &DebateErrorKind::Transient("oom".to_string()),
        0,
        true
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

#[cfg(unix)]
#[test]
fn resolve_debate_tool_auto_skips_counterpart_without_configured_binary() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let td = tempfile::tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.lock().expect("debate env lock poisoned");
    let bin_dir = td.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");

    let which_path = bin_dir.join("which");
    fs::write(
        &which_path,
        "#!/bin/sh\nif [ \"$1\" = \"codex-acp\" ]; then\n  exit 0\nfi\nexit 1\n",
    )
    .expect("write which stub");
    let mut perms = fs::metadata(&which_path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&which_path, perms).expect("chmod which");

    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let patched_path = std::env::join_paths(
        std::iter::once(bin_dir.clone()).chain(std::env::split_paths(&inherited_path)),
    )
    .expect("join PATH");
    let _path_guard = EnvVarGuard::set("PATH", &patched_path);

    let mut global = GlobalConfig::default();
    global.debate.same_model_fallback = false;

    let mut cfg = project_config_with_enabled_tools(&["codex"]);
    cfg.debate = Some(ReviewConfig {
        tool: csa_config::ToolSelection::Single("auto".to_string()),
        ..Default::default()
    });
    cfg.tools
        .get_mut("codex")
        .expect("codex tool config")
        .transport = Some(ToolTransport::Cli);

    let err = resolve_debate_tool(
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
    .unwrap_err();

    assert!(
        format!("{err:#}").contains("AUTO debate tool selection failed"),
        "expected clean auto-selection failure, got: {err:#}"
    );
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

#[tokio::test]
async fn handle_debate_persists_result_for_direct_tool_tier_rejection() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir);
    let mut config = project_config_with_enabled_tools(&["gemini-cli", "codex"]);
    config.tiers.insert(
        "default".to_string(),
        csa_config::config::TierConfig {
            description: "Test tier".to_string(),
            models: vec!["gemini-cli/google/default/xhigh".to_string()],
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    write_debate_project_config(project_dir.path(), &config);
    install_pattern(project_dir.path(), "debate");

    let cd = project_dir.path().display().to_string();
    let args = parse_debate_args(&[
        "csa",
        "debate",
        "--cd",
        &cd,
        "--tool",
        "codex",
        "Should we refactor the API?",
    ]);

    let err = handle_debate(args, 0, csa_core::types::OutputFormat::Text)
        .await
        .expect_err("direct --tool tier rejection must fail");
    assert!(
        err.chain().any(|cause| cause
            .to_string()
            .contains("restricted when tiers are configured")),
        "unexpected error chain: {err:#}"
    );

    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert_eq!(sessions.len(), 1, "expected one failed debate session");

    let result = csa_session::load_result(project_dir.path(), &sessions[0].meta_session_id)
        .unwrap()
        .expect("result.toml must be written for debate tier rejection");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("pre-exec:"));
    assert!(
        result
            .summary
            .contains("restricted when tiers are configured")
    );
}

// --- CLI parse tests for --rounds flag (#138) ---

#[test]
fn debate_cli_parses_rounds_flag() {
    let args = parse_debate_args(&["csa", "debate", "--rounds", "5", "question"]);
    assert_eq!(args.rounds, 5);
}

#[test]
fn debate_cli_parses_model_spec_and_no_failover_flags() {
    let args = parse_debate_args(&[
        "csa",
        "debate",
        "--model-spec",
        "codex/openai/gpt-5.4/xhigh",
        "--no-failover",
        "question",
    ]);
    assert_eq!(
        args.model_spec.as_deref(),
        Some("codex/openai/gpt-5.4/xhigh")
    );
    assert!(args.no_failover);
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
    // Default should follow is_terminal() on stderr
    use std::io::IsTerminal;
    let expected = if std::io::stderr().is_terminal() {
        csa_process::StreamMode::TeeToStderr
    } else {
        csa_process::StreamMode::BufferOnly
    };
    let mode = resolve_debate_stream_mode(false, false);
    assert!(matches!(mode, m if m == expected));
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

#[test]
fn render_debate_cli_output_respects_json_format() {
    use csa_core::types::OutputFormat;

    let summary = DebateSummary {
        verdict: "REVISE".to_string(),
        confidence: "medium".to_string(),
        summary: "Need more evidence.".to_string(),
        key_points: vec!["Point A".to_string()],
        mode: DebateMode::Heterogeneous,
    };

    let rendered =
        render_debate_cli_output(OutputFormat::Json, &summary, "Transcript body", "01META")
            .unwrap();
    let parsed: Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(parsed["meta_session_id"], "01META");
    assert_eq!(parsed["transcript"], "Transcript body");
}
