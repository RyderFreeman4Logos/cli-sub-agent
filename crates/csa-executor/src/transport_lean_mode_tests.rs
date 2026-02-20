use serde_json::json;

use super::AcpTransport;

#[test]
fn test_build_session_meta_with_empty_sources() {
    let sources: Vec<String> = vec![];
    let meta = AcpTransport::build_session_meta(Some(&sources)).expect("meta should exist");
    assert_eq!(
        serde_json::Value::Object(meta),
        json!({"claudeCode": {"options": {"settingSources": []}}})
    );
}

#[test]
fn test_build_session_meta_with_project_source() {
    let sources = vec!["project".to_string()];
    let meta = AcpTransport::build_session_meta(Some(&sources)).expect("meta should exist");
    assert_eq!(
        serde_json::Value::Object(meta),
        json!({"claudeCode": {"options": {"settingSources": ["project"]}}})
    );
}

#[test]
fn test_build_session_meta_none_returns_none() {
    assert!(
        AcpTransport::build_session_meta(None).is_none(),
        "meta should be absent when setting_sources is None"
    );
}
