use super::{human_session_summary, is_json_event_envelope};
use std::fs;
use tempfile::tempdir;

#[test]
fn prefers_summary_markdown_over_raw_event_envelope() {
    let temp = tempdir().expect("tempdir should be created");
    let output = temp.path().join("output");
    fs::create_dir_all(&output).expect("output dir should be created");
    fs::write(
        output.join("summary.md"),
        "PASS: no blocking findings found.\n",
    )
    .expect("summary.md should be written");
    let raw = r#"{"type":"turn.completed","usage":{"input_tokens":100}}"#;
    assert_eq!(
        human_session_summary(temp.path(), raw).as_deref(),
        Some("PASS: no blocking findings found.")
    );
}

#[test]
fn suppresses_json_event_envelope_when_no_summary_markdown() {
    let temp = tempdir().expect("tempdir should be created");
    let raw = r#"{"type":"turn.completed","usage":{"input_tokens":100}}"#;
    assert_eq!(human_session_summary(temp.path(), raw), None);
}

#[test]
fn falls_back_to_raw_prose_when_not_an_envelope() {
    let temp = tempdir().expect("tempdir should be created");
    assert_eq!(
        human_session_summary(temp.path(), "  Implemented the feature.  ").as_deref(),
        Some("Implemented the feature.")
    );
}

#[test]
fn empty_summary_markdown_falls_through_to_raw_prose() {
    let temp = tempdir().expect("tempdir should be created");
    let output = temp.path().join("output");
    fs::create_dir_all(&output).expect("output dir should be created");
    fs::write(output.join("summary.md"), "   \n").expect("summary.md should be written");
    assert_eq!(
        human_session_summary(temp.path(), "raw prose").as_deref(),
        Some("raw prose")
    );
}

#[test]
fn is_json_event_envelope_detects_typed_objects_only() {
    assert!(is_json_event_envelope(r#"{"type":"turn.completed"}"#));
    assert!(is_json_event_envelope(
        r#"  {"type":"item.completed","item":{}}  "#
    ));
    assert!(!is_json_event_envelope("PASS: no blocking findings"));
    // No `type` field -> not treated as an event envelope.
    assert!(!is_json_event_envelope(r#"{"usage":{"input_tokens":1}}"#));
    assert!(!is_json_event_envelope("42"));
    assert!(!is_json_event_envelope(""));
}
