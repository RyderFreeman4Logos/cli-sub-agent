use super::{
    compute_token_measurement, display_all_sections, display_single_section,
    display_summary_section, format_number,
};
use tempfile::tempdir;

// ── display_structured_output tests ───────────────────────────────

#[test]
fn display_summary_section_with_structured_output() {
    let tmp = tempdir().unwrap();
    let output =
        "<!-- CSA:SECTION:summary -->\nThis is the summary.\n<!-- CSA:SECTION:summary:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    // Should succeed without error
    display_summary_section(tmp.path(), "test", false).unwrap();
}

#[test]
fn display_summary_section_falls_back_to_output_log() {
    let tmp = tempdir().unwrap();
    let session_dir = tmp.path();
    // Write output.log without structured markers
    std::fs::write(session_dir.join("output.log"), "Line 1\nLine 2\nLine 3\n").unwrap();

    // Should succeed (falls back to output.log)
    display_summary_section(session_dir, "test", false).unwrap();
}

#[test]
fn display_summary_section_handles_no_output() {
    let tmp = tempdir().unwrap();
    // No output.log, no index.toml — should print message to stderr
    display_summary_section(tmp.path(), "test", false).unwrap();
}

#[test]
fn display_single_section_returns_content() {
    let tmp = tempdir().unwrap();
    let output = "<!-- CSA:SECTION:details -->\nDetail content\n<!-- CSA:SECTION:details:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    display_single_section(tmp.path(), "test", "details", false).unwrap();
}

#[test]
fn display_single_section_errors_on_missing_id() {
    let tmp = tempdir().unwrap();
    let output = "<!-- CSA:SECTION:summary -->\nContent\n<!-- CSA:SECTION:summary:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    let err = display_single_section(tmp.path(), "test", "nonexistent", false).unwrap_err();
    assert!(err.to_string().contains("not found"));
    assert!(err.to_string().contains("summary")); // lists available sections
}

#[test]
fn display_single_section_errors_when_no_structured_output() {
    let tmp = tempdir().unwrap();
    let err = display_single_section(tmp.path(), "test", "any", false).unwrap_err();
    assert!(err.to_string().contains("No structured output"));
}

#[test]
fn display_all_sections_shows_all_in_order() {
    let tmp = tempdir().unwrap();
    let output = "<!-- CSA:SECTION:intro -->\nIntro\n<!-- CSA:SECTION:intro:END -->\n\
                   <!-- CSA:SECTION:body -->\nBody\n<!-- CSA:SECTION:body:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    display_all_sections(tmp.path(), "test", false).unwrap();
}

#[test]
fn display_all_sections_falls_back_to_output_log() {
    let tmp = tempdir().unwrap();
    let session_dir = tmp.path();
    std::fs::write(session_dir.join("output.log"), "raw output here\n").unwrap();

    display_all_sections(session_dir, "test", false).unwrap();
}

// ── format_number tests ───────────────────────────────────────────

#[test]
fn format_number_small_values() {
    assert_eq!(format_number(0), "0");
    assert_eq!(format_number(42), "42");
    assert_eq!(format_number(999), "999");
}

#[test]
fn format_number_with_commas() {
    assert_eq!(format_number(1000), "1,000");
    assert_eq!(format_number(3456), "3,456");
    assert_eq!(format_number(1234567), "1,234,567");
}

// ── compute_token_measurement tests ───────────────────────────────

#[test]
fn measure_structured_output_with_summary() {
    let tmp = tempdir().unwrap();
    let output = "<!-- CSA:SECTION:summary -->\n\
                   Summary line one.\n\
                   Summary line two.\n\
                   <!-- CSA:SECTION:summary:END -->\n\
                   <!-- CSA:SECTION:analysis -->\n\
                   Analysis paragraph one with many words to increase token count.\n\
                   Analysis paragraph two with additional detail and explanation.\n\
                   <!-- CSA:SECTION:analysis:END -->\n\
                   <!-- CSA:SECTION:details -->\n\
                   Detailed implementation notes with code examples and references.\n\
                   More detail lines for testing purposes.\n\
                   <!-- CSA:SECTION:details:END -->\n\
                   <!-- CSA:SECTION:implementation -->\n\
                   Implementation code and final notes.\n\
                   <!-- CSA:SECTION:implementation:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    let m = compute_token_measurement(tmp.path(), "01TEST123").unwrap();
    assert!(m.is_structured);
    assert_eq!(m.section_count, 4);
    assert_eq!(
        m.section_names,
        vec!["summary", "analysis", "details", "implementation"]
    );
    assert!(m.summary_tokens > 0);
    assert!(m.total_tokens > m.summary_tokens);
    assert!(m.savings_percent > 0.0);
    assert_eq!(m.savings_tokens, m.total_tokens - m.summary_tokens);
}

#[test]
fn measure_unstructured_output_no_savings() {
    let tmp = tempdir().unwrap();
    let output = "Plain text without any markers.\nSecond line.\nThird line.";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    let m = compute_token_measurement(tmp.path(), "01TEST456").unwrap();
    assert!(!m.is_structured);
    assert_eq!(m.section_count, 1);
    assert_eq!(m.section_names, vec!["full"]);
    // For unstructured, summary_tokens = first section = total
    assert_eq!(m.summary_tokens, m.total_tokens);
    assert_eq!(m.savings_tokens, 0);
    assert_eq!(m.savings_percent, 0.0);
}

#[test]
fn measure_empty_output() {
    let tmp = tempdir().unwrap();
    csa_session::persist_structured_output(tmp.path(), "").unwrap();

    let m = compute_token_measurement(tmp.path(), "01EMPTY").unwrap();
    assert!(!m.is_structured);
    assert_eq!(m.total_tokens, 0);
    assert_eq!(m.summary_tokens, 0);
    assert_eq!(m.savings_tokens, 0);
    assert_eq!(m.savings_percent, 0.0);
}

#[test]
fn measure_no_index_falls_back_to_output_log() {
    let tmp = tempdir().unwrap();
    let session_dir = tmp.path();
    std::fs::write(
        session_dir.join("output.log"),
        "Some raw output content here.\n",
    )
    .unwrap();

    let m = compute_token_measurement(session_dir, "01NOINDEX").unwrap();
    assert!(!m.is_structured);
    assert!(m.total_tokens > 0);
    assert_eq!(m.summary_tokens, m.total_tokens);
    assert_eq!(m.savings_tokens, 0);
    assert!(m.section_names.is_empty());
}

#[test]
fn measure_no_output_at_all() {
    let tmp = tempdir().unwrap();
    let m = compute_token_measurement(tmp.path(), "01NOTHING").unwrap();
    assert!(!m.is_structured);
    assert_eq!(m.total_tokens, 0);
    assert_eq!(m.savings_tokens, 0);
}

#[test]
fn measure_single_named_section_is_structured() {
    let tmp = tempdir().unwrap();
    let output =
        "<!-- CSA:SECTION:report -->\nReport content here.\n<!-- CSA:SECTION:report:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    let m = compute_token_measurement(tmp.path(), "01SINGLE").unwrap();
    // Single section that is NOT "full" counts as structured
    assert!(m.is_structured);
    assert_eq!(m.section_count, 1);
    assert_eq!(m.section_names, vec!["report"]);
}
