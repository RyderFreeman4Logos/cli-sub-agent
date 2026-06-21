use super::{
    build_all_sections_json_payload, build_gate_aware_summary_content, build_result_json_payload,
    build_summary_section_json_payload, gate_summary_employee_section,
    load_structured_post_exec_gate_report, structured_sections_with_gate_first,
};
use csa_session::{SessionResult, SessionResultView};

fn issue_2311_gate_report() -> csa_session::PostExecGateReport {
    csa_session::PostExecGateReport::from_redacted_gate_output(
        "just pre-commit",
        1,
        "running just monolith-test\n\
         FAIL [   0.005s] cli-sub-agent session_cmds_result::issue_2311\n\
         error: Recipe `monolith-test` failed on line 123 with exit code 1\n",
    )
}

#[test]
fn issue_2311_summary_text_prefers_post_exec_gate_over_employee_success() {
    let report = issue_2311_gate_report();
    let employee_summary =
        "Implemented the fix, committed it, and just pre-commit passed. Working tree clean.";

    let content = build_gate_aware_summary_content(&report, Some(("summary", employee_summary)));

    assert!(
        content.starts_with(csa_session::GATE_SUMMARY_LEAD),
        "summary must lead with gate verdict: {content}"
    );
    assert!(content.contains("phase=post-exec"));
    assert!(content.contains("command=just pre-commit"));
    assert!(content.contains("step=just monolith-test"));
    assert!(content.contains("employee self-report SUPERSEDED by gate verdict"));
    assert!(content.contains("Superseded employee self-report (summary):"));
    assert!(content.contains(employee_summary));
    let gate_pos = content.find(csa_session::GATE_SUMMARY_LEAD).unwrap();
    let employee_pos = content.find(employee_summary).unwrap();
    assert!(
        gate_pos < employee_pos,
        "employee self-report must be subordinate: {content}"
    );
}

#[test]
fn issue_2311_summary_json_reports_gate_and_subordinates_employee_success() {
    let report = issue_2311_gate_report();
    let employee_summary =
        "Implemented the fix, committed it, and just pre-commit passed. Working tree clean.";

    let payload = build_summary_section_json_payload(
        Some(("summary", employee_summary)),
        None,
        Some(&report),
    )
    .unwrap();

    assert_eq!(payload["section"], "post-exec-gate");
    assert!(
        payload["content"]
            .as_str()
            .unwrap()
            .starts_with(csa_session::GATE_SUMMARY_LEAD),
        "json summary must lead with gate verdict: {payload}"
    );
    assert_eq!(payload["post_exec_gate"]["gate_command"], "just pre-commit");
    assert_eq!(payload["post_exec_gate"]["exit_code"], 1);
    assert_eq!(
        payload["post_exec_gate"]["failing_step"],
        "just monolith-test"
    );
    assert_eq!(
        payload["superseded_employee_self_report"]["section"],
        "summary"
    );
    assert_eq!(
        payload["superseded_employee_self_report"]["content"],
        employee_summary
    );
}

#[test]
fn issue_2311_full_json_puts_post_exec_gate_section_first() {
    let report = issue_2311_gate_report();
    let employee_summary =
        "Implemented the fix, committed it, and just pre-commit passed. Working tree clean.";
    let sections = vec![(
        csa_session::OutputSection {
            id: "summary".to_string(),
            title: "Summary".to_string(),
            line_start: 1,
            line_end: 1,
            token_estimate: csa_session::estimate_tokens(employee_summary),
            file_path: Some("summary.md".to_string()),
        },
        employee_summary.to_string(),
    )];

    let rendered = structured_sections_with_gate_first(&sections, Some(&report));
    let payload = build_all_sections_json_payload(&rendered).unwrap();
    let json_sections = payload["sections"].as_array().unwrap();

    assert_eq!(json_sections[0]["section"], "post-exec-gate");
    assert!(
        json_sections[0]["content"]
            .as_str()
            .unwrap()
            .starts_with(csa_session::GATE_SUMMARY_LEAD),
        "full json first section must be gate verdict: {payload}"
    );
    assert_eq!(json_sections[0]["post_exec_gate"]["exit_code"], 1);
    assert_eq!(
        payload["post_exec_gate"]["failing_step"],
        "just monolith-test"
    );
    assert_eq!(json_sections[1]["section"], "summary");
    assert_eq!(json_sections[1]["content"], employee_summary);
}

#[test]
fn issue_2311_result_json_summary_prefers_post_exec_gate() {
    let now = chrono::Utc::now();
    let employee_summary =
        "Implemented the fix, committed it, and just pre-commit passed. Working tree clean.";
    let result = SessionResult {
        post_exec_gate: Some(issue_2311_gate_report()),
        status: "failure".to_string(),
        exit_code: 1,
        summary: employee_summary.to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };

    let payload = build_result_json_payload(
        &SessionResultView {
            envelope: result,
            manager_sidecar: None,
            legacy_sidecar: None,
        },
        None,
        None,
        None,
    )
    .unwrap();

    assert!(
        payload["summary"]
            .as_str()
            .unwrap()
            .starts_with(csa_session::GATE_SUMMARY_LEAD),
        "result json summary must lead with gate verdict: {payload}"
    );
    assert_eq!(
        payload["post_exec_gate"]["failing_step"],
        "just monolith-test"
    );
    assert_eq!(payload["superseded_employee_summary"], employee_summary);
}

#[test]
fn issue_2311_summary_text_does_not_append_full_fallback_transcript() {
    let tmp = tempfile::tempdir().unwrap();
    let long_full = (1..=30)
        .map(|i| format!("raw full transcript line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    csa_session::persist_structured_output(tmp.path(), &long_full).unwrap();

    let now = chrono::Utc::now();
    let report = issue_2311_gate_report();
    let result = SessionResult {
        post_exec_gate: Some(report),
        status: "failure".to_string(),
        exit_code: 1,
        summary: "employee claimed success before gate".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };
    std::fs::write(
        tmp.path().join(csa_session::result::RESULT_FILE_NAME),
        toml::to_string(&result).unwrap(),
    )
    .unwrap();

    assert!(
        csa_session::read_section(tmp.path(), "summary")
            .unwrap()
            .is_none()
    );
    let full = csa_session::read_section(tmp.path(), "full")
        .unwrap()
        .unwrap();
    let persisted_report = load_structured_post_exec_gate_report(tmp.path()).unwrap();
    let content = build_gate_aware_summary_content(
        &persisted_report,
        gate_summary_employee_section("full", &full),
    );

    assert!(content.starts_with(csa_session::GATE_SUMMARY_LEAD));
    assert!(!content.contains("Superseded employee self-report (full):"));
    assert!(!content.contains("raw full transcript line 1"));
    assert!(!content.contains("raw full transcript line 21"));
}
