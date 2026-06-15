use super::*;

#[test]
fn build_result_json_payload_includes_kill_diagnostics() {
    let now = chrono::Utc::now();
    let result = SessionResultView {
        envelope: SessionResult {
            post_exec_gate: None,
            status: "signal".to_string(),
            exit_code: 143,
            summary: "CSA diagnostic: signal kill hint: memory soft limit".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
            kill_hint: Some("memory_soft_limit".to_string()),
            kill_diagnostics: Some(csa_session::KillDiagnosticReport {
                source: "memory_soft_limit".to_string(),
                signal: Some(15),
                current_mb: Some(9216),
                threshold_mb: Some(8601),
                memory_max_mb: Some(12_288),
                soft_limit_percent: Some(70),
                scope_name: Some("csa-codex-01KTEST.scope".to_string()),
            }),
            ..Default::default()
        },
        manager_sidecar: None,
        legacy_sidecar: None,
    };

    let payload = build_result_json_payload(&result, None, None, None).unwrap();

    assert_eq!(payload["kill_hint"], "memory_soft_limit");
    assert_eq!(payload["kill_diagnostics"]["source"], "memory_soft_limit");
    assert_eq!(payload["kill_diagnostics"]["current_mb"], 9216);
    assert_eq!(payload["kill_diagnostics"]["threshold_mb"], 8601);
}
