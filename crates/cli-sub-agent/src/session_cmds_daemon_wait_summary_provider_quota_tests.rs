use chrono::Utc;

use super::render_wait_result_summary;

#[test]
fn compact_summary_surfaces_codex_usage_limit_cooldown_from_bounded_output() {
    let temp = tempfile::tempdir().expect("tempdir");
    let api_field = concat!("api", "_", "key");
    let fake_value = concat!("sk", "-", "sec", "...", "6789");
    std::fs::write(
        temp.path().join("output.log"),
        format!(
            "noise before provider output\n\
             You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage \
             to purchase more credits or try again at Jun 24th, 2026 3:39 PM. \
             {api_field}={fake_value}\n\
             raw transcript line that should not be echoed\n"
        ),
    )
    .expect("output log should be written");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "failure".to_string(),
        exit_code: 1,
        summary: r#"{"type":"turn.failed","error":"provider_error"}"#.to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(4),
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITCODEXQUOTA", &result);

    assert!(summary.contains("Summary: provider quota exhausted: Codex usage limit hit"));
    assert!(summary.contains("retry_after=try again at Jun 24th, 2026 3:39 PM"));
    assert!(summary.contains("Hint: do not retry CSA-Codex sessions until cooldown expires"));
    assert!(!summary.contains(fake_value));
    assert!(!summary.contains("raw transcript line"));
    assert!(
        summary.len() <= 2048,
        "wait summary should stay bounded: {summary}"
    );
}
