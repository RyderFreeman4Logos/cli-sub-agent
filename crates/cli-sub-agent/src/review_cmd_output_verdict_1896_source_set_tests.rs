use super::*;

#[test]
fn issue_1896_raw_empty_findings_toml_does_not_hide_physical_details_finding() {
    let session_id = "01TEST1896RAWEMPTYHIGH";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1896-raw-empty-details-high", session_id);

    fs::write(
        session_dir.join("output").join("full.md"),
        r#"PASS

```findings.toml
findings = []
```
"#,
    )
    .expect("write full.md");
    fs::write(
        session_dir.join("output").join("details.md"),
        r#"## Findings

1. [high][correctness] Physical details finding must override raw empty structured output (crates/example/src/lib.rs:9, confidence=0.90)
"#,
    )
    .expect("write details.md");
    assert!(!session_dir.join("output").join("index.toml").exists());

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    crate::review_cmd::findings_toml::persist_review_findings_toml(&project_root, &meta);

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].severity, Severity::High);
    assert_eq!(
        findings.findings[0].file_ranges[0].path,
        "crates/example/src/lib.rs"
    );
    assert_eq!(findings.findings[0].file_ranges[0].start, 9);

    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1896_raw_empty_findings_toml_with_physical_clean_details_stays_pass() {
    let session_id = "01TEST1896RAWEMPTYCLEAN";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1896-raw-empty-details-clean", session_id);

    fs::write(
        session_dir.join("output").join("full.md"),
        r#"PASS

```findings.toml
findings = []
```
"#,
    )
    .expect("write full.md");
    fs::write(
        session_dir.join("output").join("details.md"),
        "Findings: none.\n",
    )
    .expect("write details.md");
    assert!(!session_dir.join("output").join("index.toml").exists());

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    crate::review_cmd::findings_toml::persist_review_findings_toml(&project_root, &meta);

    let findings = read_findings_toml(&session_dir);
    assert!(findings.findings.is_empty());

    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert!(verdict.severity_counts.values().all(|count| *count == 0));
    assert!(verdict.failure_reason.is_none());

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
