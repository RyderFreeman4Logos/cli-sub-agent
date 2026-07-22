use super::{render_wait_result_json, render_wait_result_summary};
use chrono::Utc;

#[test]
fn compact_summary_and_json_include_require_commit_recovery() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary:
            "require-commit contract failed: no qualifying commit or tracked dirty work remains"
                .to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        require_commit_recovery: Some(csa_session::RequireCommitRecoveryDiagnostic {
            require_commit: true,
            sa_mode: Some(false),
            commit_created: false,
            dirty_worktree: true,
            changed_paths: vec!["src/lib.rs".to_string(), "README.md".to_string()],
            changed_paths_truncated: 0,
            termination_status: "signal".to_string(),
            exit_code: 143,
            termination_signal: Some(15),
            kill_hint: Some("memory_pressure".to_string()),
            blocker_summary: Some("gate=commit-policy-uncommitted".to_string()),
            suggested_recovery_action: "inspect_changed_paths_then_commit_or_revert".to_string(),
        }),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITRECOVER", &result);

    assert!(summary.contains("Status: failure"));
    assert!(summary.contains("Require-commit recovery: CONTRACT FAILURE"));
    assert!(summary.contains("dirty_tracked_worktree=true"));
    assert!(summary.contains("commit_created=false"));
    assert!(summary.contains("status=signal"));
    assert!(summary.contains("signal=15"));
    assert!(summary.contains("Dirty tracked paths: src/lib.rs, README.md"));
    assert!(summary.contains("Blocker: gate=commit-policy-uncommitted"));
    assert!(summary.contains("Recovery action: inspect_changed_paths_then_commit_or_revert"));
    assert!(summary.contains(
        "Work was applied but not committed; use fork-from to continue from this session"
    ));
    assert!(summary.contains(
        "Continuation command: csa run --fork-from 01TESTWAITRECOVER --require-commit --sa-mode false --prompt-file CONTINUATION_PROMPT.md"
    ));
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
    assert_eq!(
        recovery["blocker_summary"],
        serde_json::json!("gate=commit-policy-uncommitted")
    );
    assert_eq!(
        json["require_commit_recovery_guidance"]["continuation_command"],
        serde_json::json!(
            "csa run --fork-from 01TESTWAITRECOVER --require-commit --sa-mode false --prompt-file CONTINUATION_PROMPT.md"
        )
    );
    assert!(
        json["require_commit_recovery_guidance"]["continuation_prompt"]
            .as_str()
            .is_some_and(|prompt| prompt.contains("git status --short"))
    );
}

#[test]
fn compact_summary_and_json_include_fix_finding_recovery_sidecar() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let output_dir = temp.path().join("output");
    std::fs::create_dir_all(&output_dir).expect("output dir should be created");
    std::fs::write(
        output_dir.join("fix_finding_recovery.json"),
        r#"{
  "schema_version": 1,
  "kind": "fix_finding_failed_closed_recovery",
  "session_id": "01TESTFIXRECOVERY",
  "outcome": "failed_closed_missing_result",
  "side_effects": {
    "status": "dirty_or_committed_tracked_changes",
    "added": {"paths": [], "truncated": 0},
    "modified": {"paths": ["src/lib.rs"], "truncated": 0},
    "deleted": {"paths": [], "truncated": 0},
    "renamed": {"paths": [], "truncated": 0}
  },
  "allow_required_push_next_step": false,
  "requires_fresh_exact_head_review": true,
  "recovery_actions": [
    "inspect_git_metadata",
    "preserve_finish_or_discard_dirty_side_effects",
    "create_hook_enabled_commit_if_appropriate",
    "run_fresh_exact_head_review_before_push_or_pr"
  ],
  "git_inspection_commands": [
    "git status --short",
    "git diff",
    "git diff --staged",
    "git log --oneline -5"
  ],
  "guidance": "Inspect git metadata, preserve and finish or discard dirty/staged side effects, create a hook-enabled commit if appropriate, and run a fresh exact-head review before push/PR."
}"#,
    )
    .expect("fix-finding recovery sidecar should be written");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary: "fix-finding failed closed".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTFIXRECOVERY", &result);

    assert!(summary.contains("Fix-finding recovery: failed_closed_missing_result"));
    assert!(summary.contains("required push next-step suppressed"));
    assert!(summary.contains(
        "Side effects: repo_side_effects=dirty_or_committed_tracked_changes modified=[src/lib.rs]"
    ));
    assert!(summary.contains("hook-enabled commit"));
    assert!(summary.contains("fresh exact-head review before push/PR"));

    let json: serde_json::Value = serde_json::from_str(
        &render_wait_result_json(temp.path(), "01TESTFIXRECOVERY", &result)
            .expect("wait JSON should render"),
    )
    .expect("wait JSON should parse");
    let recovery = &json["fix_finding_recovery"];
    assert_eq!(
        recovery["allow_required_push_next_step"],
        serde_json::json!(false)
    );
    assert_eq!(
        recovery["requires_fresh_exact_head_review"],
        serde_json::json!(true)
    );
    assert_eq!(
        recovery["recovery_actions"][3],
        serde_json::json!("run_fresh_exact_head_review_before_push_or_pr")
    );
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
