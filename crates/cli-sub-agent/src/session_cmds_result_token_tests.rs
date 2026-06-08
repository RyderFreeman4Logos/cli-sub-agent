use super::{build_result_json_payload, render_token_usage_lines};
use csa_session::{SessionResult, SessionResultView, TokenUsage};

#[test]
fn token_usage_text_derives_total_from_input_and_output_when_persisted_total_conflicts() {
    let usage = conflicting_token_usage();

    let rendered = render_token_usage_lines(&usage).join("\n");

    assert!(rendered.contains("  Input:  2,329,258 tokens"));
    assert!(rendered.contains("  Output: 14,200 tokens"));
    assert!(rendered.contains("  Total:  2,343,458 tokens"));
    assert!(!rendered.contains("  Total:  97 tokens"));
}

#[test]
fn build_result_json_payload_derives_total_from_input_and_output_when_persisted_total_conflicts() {
    let result = SessionResultView {
        envelope: session_result(),
        manager_sidecar: None,
        legacy_sidecar: None,
    };
    let usage = conflicting_token_usage();

    let payload = build_result_json_payload(&result, None, None, Some(&usage)).unwrap();

    assert_eq!(payload["total_token_usage"]["input_tokens"], 2_329_258);
    assert_eq!(payload["total_token_usage"]["output_tokens"], 14_200);
    assert_eq!(payload["total_token_usage"]["total_tokens"], 2_343_458);
    assert_ne!(payload["total_token_usage"]["total_tokens"], 97);
}

fn conflicting_token_usage() -> TokenUsage {
    TokenUsage {
        input_tokens: Some(2_329_258),
        output_tokens: Some(14_200),
        total_tokens: Some(97),
        estimated_cost_usd: None,
        cache_read_input_tokens: Some(2_081_024),
    }
}

fn session_result() -> SessionResult {
    let now = chrono::Utc::now();
    SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: "review completed".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    }
}
