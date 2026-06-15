use super::*;

#[test]
fn session_result_kill_diagnostics_roundtrip() {
    let now = chrono::Utc::now();
    let result = SessionResult {
        status: "signal".to_string(),
        exit_code: 143,
        summary: "memory soft limit".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        kill_hint: Some("memory_soft_limit".to_string()),
        kill_diagnostics: Some(KillDiagnosticReport {
            source: "memory_soft_limit".to_string(),
            signal: Some(15),
            current_mb: Some(900),
            threshold_mb: Some(700),
            memory_max_mb: Some(1000),
            soft_limit_percent: Some(70),
            scope_name: Some("csa-codex-01J.scope".to_string()),
        }),
        ..Default::default()
    };

    let toml_str = toml::to_string(&result).unwrap();
    let loaded: SessionResult = toml::from_str(&toml_str).unwrap();

    let diagnostics = loaded.kill_diagnostics.expect("kill diagnostics");
    assert_eq!(diagnostics.source, "memory_soft_limit");
    assert_eq!(diagnostics.signal, Some(15));
    assert_eq!(diagnostics.current_mb, Some(900));
    assert_eq!(diagnostics.threshold_mb, Some(700));
}
