use super::*;

#[test]
fn issue_1716_failed_final_reviewer_with_synthetic_empty_findings_is_unavailable() {
    let session_id = "01TEST1716FAILEDREVIEWER000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1716-failed-reviewer", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    fs::write(
        session_dir.join("output").join(".findings.toml.synthetic"),
        "",
    )
    .expect("write synthetic marker");
    fs::write(
        session_dir.join("output").join("full.md"),
        "I'll read the workflow, then check the diff before writing findings.\n",
    )
    .expect("write setup-only output");

    let mut meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    meta.exit_code = 137;
    meta.primary_failure = Some("API key not found".to_string());
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Unavailable);
    assert_eq!(artifact.verdict_legacy, "UNAVAILABLE");
    assert_eq!(
        artifact.primary_failure.as_deref(),
        Some("API key not found")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1716_successful_zero_findings_fallback_with_prior_failure_still_passes() {
    let session_id = "01TEST1716SUCCESSFALLBACK00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1716-successful-fallback", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write real findings.toml");

    let mut meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    meta.exit_code = 0;
    meta.primary_failure = Some("QUOTA_EXHAUSTED".to_string());
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert_eq!(artifact.primary_failure.as_deref(), Some("QUOTA_EXHAUSTED"));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
