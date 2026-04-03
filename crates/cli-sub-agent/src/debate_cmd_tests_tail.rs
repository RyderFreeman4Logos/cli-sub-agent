use super::*;

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
