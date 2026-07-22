use super::*;

#[test]
fn build_result_json_payload_includes_require_commit_recovery() {
    let now = chrono::Utc::now();
    let result = SessionResultView {
        envelope: SessionResult {
            status: "failure".to_string(),
            exit_code: 1,
            summary:
                "require-commit contract failed: no qualifying commit or tracked dirty work remains"
                    .to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            require_commit_recovery: Some(csa_session::RequireCommitRecoveryDiagnostic {
                require_commit: true,
                sa_mode: Some(false),
                commit_created: false,
                dirty_worktree: true,
                changed_paths: vec!["src/lib.rs".to_string()],
                changed_paths_truncated: 0,
                termination_status: "failure".to_string(),
                exit_code: 2,
                termination_signal: None,
                kill_hint: None,
                blocker_summary: Some("summary=rustup toolchain setup failed".to_string()),
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
    assert_eq!(
        recovery["blocker_summary"],
        serde_json::json!("summary=rustup toolchain setup failed")
    );
}

#[test]
fn build_result_json_payload_with_identity_includes_require_commit_recovery_guidance() {
    let now = chrono::Utc::now();
    let result = SessionResultView {
        envelope: SessionResult {
            status: "failure".to_string(),
            exit_code: 1,
            summary:
                "require-commit contract failed: no qualifying commit or tracked dirty work remains"
                    .to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            require_commit_recovery: Some(csa_session::RequireCommitRecoveryDiagnostic {
                require_commit: true,
                sa_mode: Some(false),
                commit_created: false,
                dirty_worktree: true,
                changed_paths: vec!["src/lib.rs".to_string()],
                changed_paths_truncated: 0,
                termination_status: "failure".to_string(),
                exit_code: 2,
                termination_signal: None,
                kill_hint: None,
                blocker_summary: Some("summary=cargo check failed".to_string()),
                suggested_recovery_action: "inspect_changed_paths_then_commit_or_revert"
                    .to_string(),
            }),
            ..Default::default()
        },
        manager_sidecar: None,
        legacy_sidecar: None,
    };
    let temp = tempfile::tempdir().expect("tempdir");

    let payload = build_result_json_payload_with_identity(
        "01KW641KP78VR43SCKJVN6HGDN",
        temp.path(),
        &result,
        None,
        None,
        None,
    )
    .unwrap();

    assert_eq!(
        payload["require_commit_recovery_guidance"]["continuation_command"],
        serde_json::json!(
            "csa run --fork-from 01KW641KP78VR43SCKJVN6HGDN --require-commit --sa-mode false --prompt-file CONTINUATION_PROMPT.md"
        )
    );
    assert!(
        payload["require_commit_recovery_guidance"]["recovery_note"]
            .as_str()
            .is_some_and(|note| note
                .contains("Work was applied but not committed; use fork-from to continue"))
    );
    assert!(
        payload["require_commit_recovery_guidance"]["continuation_prompt"]
            .as_str()
            .is_some_and(|prompt| prompt.contains("git diff --staged"))
    );
}
