use super::{render_wait_result_json, render_wait_result_summary};
use chrono::Utc;

#[test]
fn compact_summary_and_json_include_require_commit_recovery() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary: "writer session ended with uncommitted changes (--require-commit set)".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        require_commit_recovery: Some(csa_session::RequireCommitRecoveryDiagnostic {
            require_commit: true,
            commit_created: false,
            dirty_worktree: true,
            changed_paths: vec!["src/lib.rs".to_string(), "README.md".to_string()],
            changed_paths_truncated: 0,
            termination_status: "signal".to_string(),
            exit_code: 143,
            termination_signal: Some(15),
            kill_hint: Some("memory_pressure".to_string()),
            suggested_recovery_action: "inspect_changed_paths_then_commit_or_revert".to_string(),
        }),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITRECOVER", &result);

    assert!(summary.contains("Status: failure"));
    assert!(summary.contains("Require-commit recovery: CONTRACT FAILURE"));
    assert!(summary.contains("dirty_worktree=true"));
    assert!(summary.contains("commit_created=false"));
    assert!(summary.contains("status=signal"));
    assert!(summary.contains("signal=15"));
    assert!(summary.contains("Changed paths: src/lib.rs, README.md"));
    assert!(summary.contains("Recovery action: inspect_changed_paths_then_commit_or_revert"));
    assert!(!summary.contains("Review verdict: PASS"));

    let json: serde_json::Value = serde_json::from_str(
        &render_wait_result_json(temp.path(), "01TESTWAITRECOVER", &result)
            .expect("wait JSON should render"),
    )
    .expect("wait JSON should parse");
    let recovery = &json["require_commit_recovery"];
    assert_eq!(recovery["require_commit"], serde_json::json!(true));
    assert_eq!(recovery["commit_created"], serde_json::json!(false));
    assert_eq!(recovery["dirty_worktree"], serde_json::json!(true));
    assert_eq!(
        recovery["changed_paths"][0],
        serde_json::json!("src/lib.rs")
    );
    assert_eq!(recovery["termination_status"], serde_json::json!("signal"));
    assert_eq!(recovery["exit_code"], serde_json::json!(143));
    assert_eq!(recovery["termination_signal"], serde_json::json!(15));
}

#[test]
fn compact_summary_omits_require_commit_recovery_when_absent() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "done".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(1),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITNORECOVER", &result);

    assert!(!summary.contains("Require-commit recovery"));
    assert!(!summary.contains("CONTRACT FAILURE"));
}
