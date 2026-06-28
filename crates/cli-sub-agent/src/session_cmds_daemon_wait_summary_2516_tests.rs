use super::*;
use chrono::Utc;

#[test]
fn issue_2516_fail_wait_summary_includes_first_finding_and_fix_route() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output_dir = temp.path().join("output");
    std::fs::create_dir_all(&output_dir).expect("output dir should be created");
    std::fs::write(
        output_dir.join("review-verdict.json"),
        r#"{"schema_version":1,"session_id":"01TEST2516FAILSUMMARY","timestamp":"2026-04-01T00:00:00Z","decision":"fail","verdict_legacy":"HAS_ISSUES","severity_counts":{"critical":0,"high":1,"medium":0,"low":0},"prior_round_refs":[]}"#,
    )
    .expect("review verdict should be written");
    std::fs::write(
        output_dir.join("findings.toml"),
        r#"[[findings]]
id = "F1"
severity = "high"
description = "[correctness] parser accepts positive verification prose as a finding"

[[findings.file_ranges]]
path = "src/lib.rs"
start = 42
"#,
    )
    .expect("findings.toml should be written");
    std::fs::write(
        output_dir.join("suggestion.toml"),
        "[suggestion]\naction = \"confirm_then_fix_finding\"\ncommand_template = \"csa review --fix-finding --session 01TEST2516FAILSUMMARY --prompt-file <path>\"\n",
    )
    .expect("suggestion.toml should be written");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "failure".to_string(),
        exit_code: 1,
        summary: "FAIL".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TEST2516FAILSUMMARY", &result);

    assert!(summary.contains("Review verdict: FAIL"), "{summary}");
    assert!(
        summary.contains("First finding: severity=HIGH"),
        "{summary}"
    );
    assert!(summary.contains("category=correctness"), "{summary}");
    assert!(summary.contains("location=src/lib.rs:42"), "{summary}");
    assert!(
        summary.contains("csa review --fix-finding --session 01TEST2516FAILSUMMARY"),
        "{summary}"
    );
    assert!(!summary.contains("pr-bot"), "{summary}");

    let rendered = render_wait_result_json(temp.path(), "01TEST2516FAILSUMMARY", &result)
        .expect("wait result JSON should render");
    let value: serde_json::Value =
        serde_json::from_str(&rendered).expect("wait result JSON should parse");
    assert_eq!(
        value["review_failure_context"]["first_finding"]["severity"],
        "HIGH"
    );
    assert!(
        value["review_failure_context"]["fix_route"]
            .as_str()
            .unwrap()
            .contains("--fix-finding")
    );
}
