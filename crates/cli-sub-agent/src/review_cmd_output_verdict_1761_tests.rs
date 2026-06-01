use super::*;

fn read_verdict(session_dir: &Path) -> ReviewVerdictArtifact {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
        .expect("parse verdict")
}

#[test]
fn clean_codex_review_no_findings_maps_to_pass() {
    let session_id = "01TEST1761CLEANNOFINDINGS";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1761-clean-no-findings", session_id);

    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
Reviewed main...HEAD in read-only mode.
<!-- CSA:SECTION:summary:END -->
<!-- CSA:SECTION:details -->
No blocking findings.

Notes:
- I did not run the test suite because this CSA subprocess is read-only.
- Codegraph was unavailable because this checkout has no initialized index.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert!(
        verdict.severity_counts.values().all(|count| *count == 0),
        "clean no-findings review must keep zero severity counts"
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn codex_review_with_blocking_finding_maps_to_fail() {
    let session_id = "01TEST1761BLOCKINGFINDING";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1761-blocking-finding", session_id);

    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
Blocking review finding found.
<!-- CSA:SECTION:summary:END -->
<!-- CSA:SECTION:details -->
High: crates/cli-sub-agent/src/review_cmd.rs:10 merge gate accepts a false pass.

Notes:
- Codegraph was unavailable, but the blocking finding is explicit.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_ne!(verdict.decision, ReviewDecision::Pass);
    assert_ne!(verdict.decision, ReviewDecision::Unavailable);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
