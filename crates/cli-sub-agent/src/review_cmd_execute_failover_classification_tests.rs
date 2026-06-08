use super::super::*;

#[test]
fn classify_review_failover_reason_uses_summary_http_400_for_review_fallback() {
    let execution = crate::pipeline::SessionExecutionResult {
        execution: csa_process::ExecutionResult {
            output: String::new(),
            stderr_output: String::new(),
            summary: "Gemini request failed: status: 400 Bad Request".to_string(),
            exit_code: 1,
            peak_memory_mb: None,
            ..Default::default()
        },
        meta_session_id: "01TESTHTTP400SUMMARY".to_string(),
        provider_session_id: None,
        changed_paths: None,
    };

    let failure = classify_review_failover_reason(
        ToolName::GeminiCli,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        &execution,
        None,
        Some(std::time::Duration::from_secs(2)),
    )
    .expect("HTTP 400 summary should be failover eligible during init window");

    assert_eq!(failure.reason, "HTTP 400");
    assert_eq!(failure.quota_exhausted, Some(false));
}
