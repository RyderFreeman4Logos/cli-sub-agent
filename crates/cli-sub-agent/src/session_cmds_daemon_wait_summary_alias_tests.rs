use chrono::Utc;

use super::{render_wait_result_json, render_wait_result_summary};

#[test]
fn compact_summary_and_json_expose_resume_wrapper_alias() {
    let temp = tempfile::tempdir().expect("tempdir");
    let wrapper_id = csa_session::new_session_id();
    let target_id = csa_session::new_session_id();
    let target_dir = temp.path().join(&target_id);
    std::fs::create_dir_all(&target_dir).expect("target session dir should be created");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "done".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        ..Default::default()
    };

    let summary = render_wait_result_summary(&target_dir, &wrapper_id, &result);

    assert!(summary.contains(&format!("Session: {wrapper_id}")));
    assert!(summary.contains(&format!("Target session: {target_id}")));
    assert!(summary.contains(&format!(
        "Alias: kind=resume-wrapper requested_session_id={wrapper_id} target_session_id={target_id}"
    )));
    assert!(!summary.contains(&format!("Session: {target_id}")));

    let rendered = render_wait_result_json(&target_dir, &wrapper_id, &result)
        .expect("wait result JSON should render");
    let value: serde_json::Value =
        serde_json::from_str(&rendered).expect("wait result JSON should parse");
    assert_eq!(value["session_id"].as_str(), Some(wrapper_id.as_str()));
    assert_eq!(
        value["target_session_id"].as_str(),
        Some(target_id.as_str())
    );
    assert_eq!(value["alias"]["kind"].as_str(), Some("resume-wrapper"));
    assert_eq!(
        value["alias"]["requested_session_id"].as_str(),
        Some(wrapper_id.as_str())
    );
    assert_eq!(
        value["alias"]["target_session_id"].as_str(),
        Some(target_id.as_str())
    );
}
