use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_session::{SessionResult, create_session, save_result};
use tempfile::tempdir;

fn write_success_result_for(project_root: &Path, session_id: &str) {
    let now = chrono::Utc::now();
    let result = SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "task completed".to_string(),
        tool: "claude-code".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: vec![],
        peak_memory_mb: None,
        fallback_chain: None,
        gate_timeout: false,
        manager_fields: Default::default(),
    };
    save_result(project_root, session_id, &result).unwrap();
}

#[test]
fn overwrites_success_result_with_failure() {
    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();
    let session =
        create_session(project_root, Some("gate-test"), None, Some("claude-code")).unwrap();
    let session_id = &session.meta_session_id;
    write_success_result_for(project_root, session_id);

    let initial = csa_session::load_result(project_root, session_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        initial.exit_code, 0,
        "precondition: result.toml must start as success"
    );

    let gate_err = anyhow::anyhow!("just pre-commit: No justfile found (exit=1)");
    overwrite_result_as_post_exec_gate_failure(project_root, session_id, &gate_err);

    let result = csa_session::load_result(project_root, session_id)
        .unwrap()
        .expect("result.toml should still exist");
    assert_eq!(result.exit_code, 1, "exit_code must be overwritten to 1");
    assert_eq!(
        result.status, "failure",
        "status must be overwritten to failure"
    );
    assert!(
        result.summary.contains("post-exec gate failed"),
        "summary must reference gate failure, got: {}",
        result.summary
    );
}

#[test]
fn no_panic_when_session_missing() {
    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();
    let gate_err = anyhow::anyhow!("gate failed");
    // Must not panic even when result.toml does not exist
    overwrite_result_as_post_exec_gate_failure(
        project_root,
        "01TESTMISSINGSESSION0000000A",
        &gate_err,
    );
}
