use super::*;

/// #1362: synthetic-empty findings.toml must not force the verdict pipeline
/// back to stale fail meta when the persisted review JSON and summary both say clean.
#[test]
fn issue_1362_empty_consolidated_findings_and_pass_summary_emits_pass() {
    let session_id = "01TEST1362PASSZEROFINDS000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1362-pass-zero-findings", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write synthetic findings.toml");
    fs::write(
        session_dir.join("output").join(".findings.toml.synthetic"),
        "",
    )
    .expect("write synthetic marker");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nPASS\n\nNo serious issues found.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist pass summary");

    let review_artifact = json!({
        "findings": [],
        "severity_summary": SeveritySummary { critical: 0, high: 0, medium: 0, low: 0 },
        "review_mode": "standard",
        "schema_version": "1.0",
        "session_id": session_id,
        "timestamp": chrono::Utc::now()
    });
    fs::write(
        session_dir.join(crate::bug_class::CONSOLIDATED_REVIEW_ARTIFACT_FILE),
        serde_json::to_vec_pretty(&review_artifact).expect("serialize review artifact"),
    )
    .expect("write consolidated review artifact");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "#1362: empty review JSON + PASS summary must emit pass, not stale fail meta"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|count| *count == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1362_consolidated_blocking_finding_still_fails() {
    let session_id = "01TEST1362HIGHFINDING00000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1362-high-finding", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write synthetic findings.toml");
    fs::write(
        session_dir.join("output").join(".findings.toml.synthetic"),
        "",
    )
    .expect("write synthetic marker");

    let review_artifact = json!({
        "findings": [make_finding(Severity::High, "real-high")],
        "severity_summary": SeveritySummary { critical: 0, high: 1, medium: 0, low: 0 },
        "review_mode": "standard",
        "schema_version": "1.0",
        "session_id": session_id,
        "timestamp": chrono::Utc::now()
    });
    fs::write(
        session_dir.join(crate::bug_class::CONSOLIDATED_REVIEW_ARTIFACT_FILE),
        serde_json::to_vec_pretty(&review_artifact).expect("serialize review artifact"),
    )
    .expect("write consolidated review artifact");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
