fn read_verdict(session_dir: &Path) -> ReviewVerdictArtifact {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    serde_json::from_str(&fs::read_to_string(verdict_path).expect("read verdict"))
        .expect("parse verdict")
}

#[test]
fn issue_2393_pass_summary_without_findings_artifact_overrides_stale_fail_meta() {
    let session_id = "01TEST2393PASSSUMMARY000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2393-pass-summary-stale-fail-meta", session_id);

    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
PASS No blocking correctness, security, or AGENTS.md compliance findings found for `range:main...HEAD`.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
No blocking findings found.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let mut meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    meta.exit_code = 1;
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert!(verdict.severity_counts.values().all(|count| *count == 0));
    assert!(verdict.failure_reason.is_none());

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
