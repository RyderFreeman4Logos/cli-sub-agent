use super::{enrich_review_summary, human_session_summary, is_json_event_envelope};
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
fn prefers_summary_markdown_over_thread_started_envelope() {
    // Spec test #4: a session whose persisted `summary` is the raw codex
    // `thread.started` event envelope, but which has an `output/summary.md`,
    // must render the human-authored markdown rather than the JSON envelope
    // when the result is displayed as text (#161).
    let temp = tempdir().expect("tempdir should be created");
    let output = temp.path().join("output");
    fs::create_dir_all(&output).expect("output dir should be created");
    fs::write(
        output.join("summary.md"),
        "Implemented the gate classifier and added regression tests.\n",
    )
    .expect("summary.md should be written");
    let raw = r#"{"type":"thread.started","thread_id":"thread_1"}"#;
    assert_eq!(
        human_session_summary(temp.path(), raw).as_deref(),
        Some("Implemented the gate classifier and added regression tests.")
    );
}

#[test]
fn suppresses_thread_started_envelope_when_no_summary_markdown() {
    // Negative half of spec test #4: without an `output/summary.md`, the raw
    // `thread.started` envelope must be suppressed (return None) so the display
    // path never prints machine JSON as the human summary.
    let temp = tempdir().expect("tempdir should be created");
    let raw = r#"{"type":"thread.started","thread_id":"thread_1"}"#;
    assert_eq!(human_session_summary(temp.path(), raw), None);
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
fn enriches_bare_fail_verdict_with_backtick_high_finding() {
    let temp = tempdir().expect("tempdir should be created");
    csa_session::persist_structured_output(
        temp.path(),
        "<!-- CSA:SECTION:details -->\n1. `[HIGH]` Untracked-file line counting can block prompt assembly on special files.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist structured output");

    assert_eq!(
        enrich_review_summary(temp.path(), "FAIL"),
        "FAIL — [HIGH] Untracked-file line counting can block prompt assembly on special files"
    );
}

#[test]
fn enriches_with_highest_severity_finding_when_multiple() {
    let temp = tempdir().expect("tempdir should be created");
    csa_session::persist_structured_output(
        temp.path(),
        "<!-- CSA:SECTION:details -->\n1. `[MEDIUM]` minor nit in logging.\n2. `[CRITICAL][correctness]` data loss on crash.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist structured output");

    assert_eq!(
        enrich_review_summary(temp.path(), "FAIL"),
        "FAIL — [CRITICAL] data loss on crash"
    );
}

#[test]
fn does_not_enrich_passing_verdict_even_with_stale_findings() {
    let temp = tempdir().expect("tempdir should be created");
    csa_session::persist_structured_output(
        temp.path(),
        "<!-- CSA:SECTION:details -->\n1. `[HIGH]` historical finding from an earlier fix round.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist structured output");

    assert_eq!(enrich_review_summary(temp.path(), "PASS"), "PASS");
}

#[test]
fn fails_open_to_bare_verdict_when_no_legible_finding() {
    let temp = tempdir().expect("tempdir should be created");
    csa_session::persist_structured_output(
        temp.path(),
        "<!-- CSA:SECTION:details -->\nNo machine-readable findings were emitted by the reviewer.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist structured output");

    assert_eq!(enrich_review_summary(temp.path(), "FAIL"), "FAIL");
}

#[test]
fn leaves_non_verdict_prose_summary_unchanged() {
    let temp = tempdir().expect("tempdir should be created");
    assert_eq!(
        enrich_review_summary(temp.path(), "Implemented the feature and added tests."),
        "Implemented the feature and added tests."
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
