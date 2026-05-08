use super::*;

// ─── #1349 regression tests: Fail meta must not override empty findings ────

/// #1349 Case 1: meta.decision=fail + empty findings.toml → decision=pass.
///
/// Regression: 4 consecutive reviews on feat/1346-quota-fallback produced
/// verdict=fail despite findings=[], because `derive_decision_from_severity_counts`
/// reached the prose tiebreak with meta_decision=Fail and prose that contained
/// no explicit PASS/CLEAN phrase, causing it to return Fail.
///
/// Fix: empty findings + zero severity counts is conclusive Pass, regardless
/// of meta_decision or prose content.
#[test]
fn issue_1349_fail_meta_with_empty_findings_toml_emits_pass() {
    let session_id = "01TEST1349FAILEMPTYFINDS000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1349-fail-empty-findings-toml", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    // No clean-phrase prose — the bug reproduced when prose was neutral/absent
    // (e.g. just a diff description, no PASS/CLEAN token)

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "#1349: Fail meta + empty findings.toml + zero counts must yield Pass"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|v| *v == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1349 Case 2: meta.decision=fail + empty findings.toml + neutral prose → pass.
///
/// Validates the most common failure pattern from the issue: review output
/// says something neutral (e.g. "Semantics-preserving refactor") and exits
/// non-zero, producing meta.decision=Fail, but findings.toml has no findings.
#[test]
fn issue_1349_fail_meta_with_empty_findings_toml_neutral_prose_emits_pass() {
    let session_id = "01TEST1349FAILNEUTRALPRSE00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1349-fail-neutral-prose", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nSemantics-preserving refactor.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist summary");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "#1349: Fail meta + empty findings.toml + neutral prose must yield Pass"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1349 Case 3: meta.decision=fail + empty review-findings.json → pass.
///
/// Same fix applies to the JSON artifact path, not just findings.toml.
#[test]
fn issue_1349_fail_meta_with_empty_json_findings_emits_pass() {
    let session_id = "01TEST1349FAILEMPTYJSON0000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1349-fail-empty-json-findings", session_id);

    let json_artifact = json!({
        "findings": [],
        "severity_summary": SeveritySummary { critical: 0, high: 0, medium: 0, low: 0 },
        "overall_risk": "low"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&json_artifact).expect("serialize"),
    )
    .expect("write review-findings.json");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "#1349: Fail meta + empty review-findings.json + zero counts must yield Pass"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|v| *v == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1349 Case 4: Fail meta with actual HIGH finding still emits Fail.
/// Verify the fix doesn't accidentally suppress real failures.
#[test]
fn issue_1349_fail_meta_with_high_finding_still_fails() {
    let session_id = "01TEST1349FAILHIGHFINDING00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1349-fail-high-finding", session_id);

    let json_artifact = json!({
        "findings": [make_finding(Severity::High, "real-high")],
        "severity_summary": SeveritySummary { critical: 0, high: 1, medium: 0, low: 0 },
        "overall_risk": "high"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&json_artifact).expect("serialize"),
    )
    .expect("write review-findings.json");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Fail,
        "#1349: real HIGH finding must still yield Fail"
    );
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1349 Case 5: Fail meta + overall_risk=high + empty findings still fails.
/// overall_risk is a stronger signal than empty findings structure.
#[test]
fn issue_1349_fail_meta_with_high_overall_risk_still_fails() {
    let session_id = "01TEST1349FAILHIGHRISK00000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1349-fail-high-overall-risk", session_id);

    let json_artifact = json!({
        "findings": [],
        "severity_summary": SeveritySummary { critical: 0, high: 0, medium: 0, low: 0 },
        "overall_risk": "high"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&json_artifact).expect("serialize"),
    )
    .expect("write review-findings.json");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Fail,
        "#1349: overall_risk=high overrides empty findings — must still yield Fail"
    );
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
