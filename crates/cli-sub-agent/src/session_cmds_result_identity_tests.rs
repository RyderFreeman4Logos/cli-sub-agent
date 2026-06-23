use super::build_result_json_payload_with_identity;
use csa_session::{SessionResult, SessionResultView};
use tempfile::tempdir;

#[test]
fn build_result_json_payload_with_identity_exposes_resume_wrapper_alias() {
    let temp = tempdir().unwrap();
    let wrapper_id = csa_session::new_session_id();
    let target_id = csa_session::new_session_id();
    let target_dir = temp.path().join(&target_id);
    std::fs::create_dir_all(&target_dir).unwrap();
    let now = chrono::Utc::now();
    let result = SessionResultView {
        envelope: SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "review completed".to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            ..Default::default()
        },
        manager_sidecar: None,
        legacy_sidecar: None,
    };

    let payload = build_result_json_payload_with_identity(
        &wrapper_id,
        &target_dir,
        &result,
        None,
        None,
        None,
    )
    .unwrap();

    assert_eq!(payload["session_id"].as_str(), Some(wrapper_id.as_str()));
    assert_eq!(
        payload["target_session_id"].as_str(),
        Some(target_id.as_str())
    );
    assert_eq!(payload["alias"]["kind"].as_str(), Some("resume-wrapper"));
    assert_eq!(
        payload["alias"]["requested_session_id"].as_str(),
        Some(wrapper_id.as_str())
    );
    assert_eq!(
        payload["alias"]["target_session_id"].as_str(),
        Some(target_id.as_str())
    );
}
