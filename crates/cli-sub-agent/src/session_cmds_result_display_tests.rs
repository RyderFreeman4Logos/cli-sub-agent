use super::{build_result_json_payload_with_identity, lines};
use csa_session::SessionResultView;

const SESSION_ID: &str = "01KW641KP78VR43SCKJVN6HGDN";
const CONTINUATION_COMMAND: &str = "csa run --fork-from 01KW641KP78VR43SCKJVN6HGDN --require-commit --build-jobs 1 --memory-max-mb 11208 --prompt-file CONTINUATION_PROMPT.md";

#[test]
fn issue_2440_session_result_shows_memory_soft_limit_guidance() {
    let result = memory_soft_limit_result();
    let recovery = result
        .envelope
        .memory_soft_limit_recovery
        .as_ref()
        .expect("memory soft-limit recovery diagnostic");
    let temp = tempfile::tempdir().expect("tempdir");
    let rendered = lines(SESSION_ID, temp.path(), &result.envelope, recovery).join("\n");

    assert!(rendered.contains(
        "Memory-soft-limit recovery: outcome=dirty_or_staged_changes dirty_worktree=true"
    ));
    assert!(rendered.contains(&format!("Continuation command: {CONTINUATION_COMMAND}")));
    assert!(rendered.contains("Continuation prompt guidance: Inspect git status --short"));
    assert!(rendered.contains("Retry guidance: Avoid blind retry under the same memory cap"));
    assert!(rendered.contains("current_mb=9626"));

    let payload =
        build_result_json_payload_with_identity(SESSION_ID, temp.path(), &result, None, None, None)
            .expect("result payload");

    assert_eq!(
        payload["memory_soft_limit_recovery_guidance"]["continuation_command"],
        CONTINUATION_COMMAND
    );
    assert!(
        payload["memory_soft_limit_recovery_guidance"]["continuation_prompt"]
            .as_str()
            .expect("continuation prompt")
            .contains("preserve existing staged and unstaged work")
    );
    assert!(
        payload["memory_soft_limit_recovery_guidance"]["retry_guidance"]
            .as_str()
            .expect("retry guidance")
            .contains("current_mb=9626")
    );
}

mod session_cmds_result_memory_soft_limit {
    use super::*;

    #[test]
    fn issue_2440_continuation_guidance_uses_worker_session_id() {
        let result = memory_soft_limit_result();
        let recovery = result
            .envelope
            .memory_soft_limit_recovery
            .as_ref()
            .expect("memory soft-limit recovery diagnostic");
        let temp = tempfile::tempdir().expect("tempdir");
        let wrapper_id = csa_session::new_session_id();
        let worker_id = csa_session::new_session_id();
        let worker_dir = temp.path().join(&worker_id);
        std::fs::create_dir_all(&worker_dir).expect("worker session dir");
        let expected_command = CONTINUATION_COMMAND.replace(SESSION_ID, &worker_id);

        let rendered = lines(&wrapper_id, &worker_dir, &result.envelope, recovery).join("\n");

        assert!(rendered.contains(&format!("Continuation command: {expected_command}")));
        assert!(
            !rendered.contains(&format!("--fork-from {wrapper_id}")),
            "continuation guidance must not resume the wrapper session: {rendered}"
        );

        let payload = build_result_json_payload_with_identity(
            &wrapper_id,
            &worker_dir,
            &result,
            None,
            None,
            None,
        )
        .expect("result payload");

        assert_eq!(payload["session_id"].as_str(), Some(wrapper_id.as_str()));
        assert_eq!(
            payload["target_session_id"].as_str(),
            Some(worker_id.as_str())
        );
        assert_eq!(
            payload["memory_soft_limit_recovery_guidance"]["continuation_command"],
            expected_command.as_str()
        );
        assert_ne!(
            payload["memory_soft_limit_recovery_guidance"]["continuation_command"]
                .as_str()
                .expect("continuation command"),
            CONTINUATION_COMMAND.replace(SESSION_ID, &wrapper_id)
        );
    }
}

fn memory_soft_limit_result() -> SessionResultView {
    let now = chrono::Utc::now();
    SessionResultView {
        envelope: csa_session::SessionResult {
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
                current_mb: Some(9626),
                threshold_mb: Some(9000),
                memory_max_mb: Some(10000),
                soft_limit_percent: Some(90),
                scope_name: Some("csa-codex-01KTEST.scope".to_string()),
            }),
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
    }
}
