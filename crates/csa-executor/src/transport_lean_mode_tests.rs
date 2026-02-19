use serde_json::json;

use super::AcpTransport;

#[test]
fn test_build_lean_mode_meta_enabled() {
    let meta = AcpTransport::build_lean_mode_meta(true).expect("meta should exist");
    assert_eq!(
        serde_json::Value::Object(meta),
        json!({"claudeCode": {"options": {"settingSources": []}}})
    );
}

#[test]
fn test_build_lean_mode_meta_disabled() {
    assert!(
        AcpTransport::build_lean_mode_meta(false).is_none(),
        "meta should be absent when lean mode is off"
    );
}
