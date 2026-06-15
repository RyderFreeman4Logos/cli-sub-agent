use super::*;

fn memory_report() -> csa_session::KillDiagnosticReport {
    csa_session::KillDiagnosticReport {
        source: "memory_soft_limit".to_string(),
        signal: Some(15),
        current_mb: Some(9216),
        threshold_mb: Some(8601),
        memory_max_mb: Some(12_288),
        soft_limit_percent: Some(70),
        scope_name: Some("csa-codex-01KTEST.scope".to_string()),
    }
}

fn signal_result(summary: String) -> csa_session::SessionResult {
    let now = Utc::now();
    csa_session::SessionResult {
        post_exec_gate: None,
        status: "signal".to_string(),
        exit_code: 143,
        summary,
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        kill_hint: Some("memory_soft_limit".to_string()),
        kill_diagnostics: Some(memory_report()),
        last_item: None,
        fallback_chain: None,
        ..Default::default()
    }
}

#[test]
fn compact_summary_includes_signal_kill_hint() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let diagnostic = "CSA diagnostic: signal kill hint: memory soft limit (termination_reason=signal, CSA memory monitor soft-limit event matched signal exit, current_mb=9216, threshold_mb=8601, memory_max_mb=12288, soft_limit_percent=70, scope_name=csa-codex-01KTEST.scope). CSA's memory monitor sent SIGTERM at the configured soft limit; increase resources.memory_max_mb or tools.<tool>.memory_max_mb, raise resources.soft_limit_percent only if safe, or reduce compile/test parallelism.";
    let result = signal_result(diagnostic.to_string());

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITKILL", &result);

    assert!(summary.contains("Kill hint: memory_soft_limit"));
    assert!(summary.contains("CSA diagnostic: signal kill hint: memory soft limit"));
    assert!(summary.contains("current_mb=9216"));
    assert!(summary.contains("threshold_mb=8601"));
}

#[test]
fn compact_json_includes_kill_diagnostics() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let result = signal_result("process killed by signal 15 (SIGTERM)".to_string());

    let rendered = render_wait_result_json(temp.path(), "01TESTWAITKILLJSON", &result)
        .expect("wait result JSON should render");
    let value: serde_json::Value =
        serde_json::from_str(&rendered).expect("wait result JSON should parse");

    assert_eq!(value["kill_hint"], "memory_soft_limit");
    assert_eq!(value["kill_diagnostics"]["source"], "memory_soft_limit");
    assert_eq!(value["kill_diagnostics"]["current_mb"], 9216);
    assert_eq!(value["kill_diagnostics"]["threshold_mb"], 8601);
}
