use super::*;

#[test]
fn build_result_json_payload_includes_require_commit_recovery() {
    let now = chrono::Utc::now();
    let result = SessionResultView {
        envelope: SessionResult {
            status: "failure".to_string(),
            exit_code: 1,
            summary: "writer session ended with uncommitted changes (--require-commit set)"
                .to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            require_commit_recovery: Some(csa_session::RequireCommitRecoveryDiagnostic {
                require_commit: true,
                commit_created: false,
                dirty_worktree: true,
                changed_paths: vec!["src/lib.rs".to_string()],
                changed_paths_truncated: 0,
                termination_status: "failure".to_string(),
                exit_code: 2,
                termination_signal: None,
                kill_hint: None,
                suggested_recovery_action: "inspect_changed_paths_then_commit_or_revert"
                    .to_string(),
            }),
            ..Default::default()
        },
        manager_sidecar: None,
        legacy_sidecar: None,
    };

    let payload = build_result_json_payload(&result, None, None, None).unwrap();
    let recovery = &payload["require_commit_recovery"];

    assert_eq!(recovery["require_commit"], serde_json::json!(true));
    assert_eq!(recovery["commit_created"], serde_json::json!(false));
    assert_eq!(recovery["dirty_worktree"], serde_json::json!(true));
    assert_eq!(
        recovery["changed_paths"][0],
        serde_json::json!("src/lib.rs")
    );
    assert_eq!(
        recovery["suggested_recovery_action"],
        serde_json::json!("inspect_changed_paths_then_commit_or_revert")
    );
}
