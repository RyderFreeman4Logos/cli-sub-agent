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

#[test]
fn build_result_json_payload_includes_memory_soft_limit_recovery() {
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
            memory_soft_limit_recovery: Some(csa_session::MemorySoftLimitRecoveryDiagnostic {
                outcome: "dirty_or_staged_changes".to_string(),
                commit_created: false,
                dirty_worktree: true,
                changed_paths: vec!["src/lib.rs".to_string()],
                changed_paths_truncated: 0,
                git_status_short: vec![" M src/lib.rs".to_string()],
                git_status_short_truncated: 0,
                head_oid: None,
                head_summary: None,
                suggested_recovery_action:
                    "inspect_git_status_preserve_changes_then_rerun_with_memory_headroom"
                        .to_string(),
                retry_profile: None,
            }),
            ..Default::default()
        },
        manager_sidecar: None,
        legacy_sidecar: None,
    };

    let payload = build_result_json_payload(&result, None, None, None).unwrap();

    assert_eq!(
        payload["memory_soft_limit_recovery"]["outcome"],
        "dirty_or_staged_changes"
    );
    assert_eq!(
        payload["memory_soft_limit_recovery"]["changed_paths"][0],
        "src/lib.rs"
    );
    assert_eq!(
        payload["memory_soft_limit_recovery"]["git_status_short"][0],
        " M src/lib.rs"
    );
}
