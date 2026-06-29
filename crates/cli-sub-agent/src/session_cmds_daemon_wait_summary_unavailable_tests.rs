use chrono::Utc;

use super::render_wait_result_summary;

fn write_review_verdict(session_dir: &std::path::Path, body: String) {
    let output_dir = session_dir.join("output");
    std::fs::create_dir_all(&output_dir).expect("output dir should be created");
    std::fs::write(output_dir.join("review-verdict.json"), body)
        .expect("review verdict should be written");
}

#[test]
fn compact_summary_includes_redacted_provider_usage_limit_reason() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let long_tail = " provider-debug".repeat(80);
    let api_field = concat!("api", "_", "key");
    let fake_value = concat!("sk", "-", "sec", "...", "6789");
    write_review_verdict(
        temp.path(),
        format!(
            r#"{{"schema_version":1,"session_id":"01TESTWAITUSAGE","timestamp":"2026-04-01T00:00:00Z","decision":"unavailable","verdict_legacy":"UNAVAILABLE","severity_counts":{{"critical":0,"high":0,"medium":0,"low":0}},"primary_failure":"HTTP 429","failure_reason":"codex/openai/gpt-5.5/xhigh=You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage to purchase more credits or try again at Jun 20th, 2026 6:48 PM. {api_field}={fake_value} {long_tail}","prior_round_refs":[]}}"#
        ),
    );
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "failed".to_string(),
        exit_code: 1,
        summary: "review unavailable".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITUSAGE", &result);

    assert!(summary.contains("Review verdict: UNAVAILABLE (HTTP 429)"));
    assert!(summary.contains("Unavailable reason: provider_usage_limit:"));
    assert!(summary.contains("You've hit your usage limit."));
    assert!(summary.contains("try again at Jun 20th, 2026 6:48 PM"));
    assert!(!summary.contains(fake_value));
    assert!(summary.contains("[REDACTED]"));
    assert!(
        summary.len() <= 2048,
        "summary should stay bounded: {summary}"
    );
}

#[test]
fn compact_summary_omits_provider_usage_reason_for_auth_only_unavailable() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let api_field = concat!("api", "_", "key");
    let fake_value = concat!("sk", "-", "sec", "...", "6789");
    write_review_verdict(
        temp.path(),
        format!(
            r#"{{"schema_version":1,"session_id":"01TESTWAITAUTHA","timestamp":"2026-04-01T00:00:00Z","decision":"unavailable","verdict_legacy":"UNAVAILABLE","severity_counts":{{"critical":0,"high":0,"medium":0,"low":0}},"primary_failure":"api_key_invalid","failure_reason":"gemini-cli tool failure: API Key not found; {api_field}={fake_value}","prior_round_refs":[]}}"#
        ),
    );
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "failed".to_string(),
        exit_code: 1,
        summary: "review unavailable".to_string(),
        tool: "gemini-cli".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITAUTHA", &result);

    assert!(!summary.contains("Unavailable reason:"));
    assert!(!summary.contains(fake_value));
}

#[test]
fn issue_2512_unavailable_primary_failure_is_redacted_in_label_and_failover() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let api_field = concat!("api", "_", "key");
    let fake_value = concat!("sk", "-", "sec", "...", "6789");
    write_review_verdict(
        temp.path(),
        format!(
            r#"{{"schema_version":1,"session_id":"01TEST2512REDACT","timestamp":"2026-06-29T00:00:00Z","decision":"unavailable","verdict_legacy":"UNAVAILABLE","severity_counts":{{"critical":0,"high":0,"medium":0,"low":0}},"primary_failure":"host memory admission denied; {api_field}={fake_value}","failure_reason":"infrastructure_unavailable","prior_round_refs":[]}}"#
        ),
    );
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "failed".to_string(),
        exit_code: 1,
        summary: "review unavailable".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        events_count: 0,
        artifacts: Vec::new(),
        fallback_chain: Some(vec![csa_core::types::FallbackAttempt {
            tool: "codex".into(),
            model_spec: None,
            skip_reason: "memory admission denied".into(),
            quota_exhausted: false,
            timestamp: now,
        }]),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TEST2512REDACT", &result);

    // The credential must never appear in the output
    assert!(
        !summary.contains(fake_value),
        "primary_failure credential leaked into wait summary: {summary}"
    );
    assert!(summary.contains("[REDACTED]"), "expected redaction marker");
}
