use super::*;

// ─── #1340/R2-001 regression tests: Unavailable meta fails closed ─

/// #1340 Case 1: meta.decision=unavailable (from text-parse noise) + empty
/// findings.toml now fails closed instead of promoting to Pass.
///
/// Regression: UNAVAILABLE token in prompt-injection text caused
/// parse_review_decision_token to return Unavailable, which then propagated
/// through derive_decision_from_severity_counts even though findings were empty.
/// R2-001 supersedes that recovery: unavailable reviewer outcome is incomplete
/// reviewer evidence, so empty findings cannot synthesize a clean artifact.
#[test]
fn issue_1340_unavailable_meta_with_empty_findings_toml_fails_closed() {
    let session_id = "01TEST1340UNAVAILABLEFINDS0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1340-unavailable-empty-findings", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    // Summary says PASS — matches the real-world session 01KR2S11132Q9WNFQ7HK3AT966
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\n**PASS** — Auto-commit fix. No blocking findings.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist summary");

    // meta.decision = "unavailable" simulates text-parse contamination by prompt injection
    let meta =
        make_review_meta_with_decision(session_id, ReviewDecision::Unavailable, "UNAVAILABLE");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Unavailable,
        "R2-001: Unavailable meta + empty findings.toml must fail closed"
    );
    assert_eq!(artifact.verdict_legacy, "UNAVAILABLE");
    assert!(artifact.severity_counts.values().all(|v| *v == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1340 Case 2: meta.decision=unavailable + empty findings.toml + no summary section
/// → fail closed (zero-findings is not proof that unavailable reviewer ran).
#[test]
fn issue_1340_unavailable_meta_with_empty_findings_toml_no_summary_fails_closed() {
    let session_id = "01TEST1340UNAVAILABLENOSUM0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1340-unavailable-no-summary", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    // No summary.md — unavailable meta must not be promoted by zero-findings alone.

    let meta =
        make_review_meta_with_decision(session_id, ReviewDecision::Unavailable, "UNAVAILABLE");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Unavailable,
        "R2-001: Unavailable meta + empty findings.toml (no summary) must fail closed"
    );
    assert_eq!(artifact.verdict_legacy, "UNAVAILABLE");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1340 Case 3: Verify genuine status_reason path still emits Unavailable.
///
/// When status_reason is set (real infrastructure failure), persist_review_verdict
/// takes the fast-path that uses meta.decision directly — Unavailable is preserved.
#[test]
fn issue_1340_status_reason_set_preserves_unavailable() {
    let session_id = "01TEST1340STATUSREASONFAIL0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1340-status-reason-unavailable", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    // findings.toml is empty (would otherwise indicate pass)
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");

    // status_reason = Some → genuine infrastructure failure → fast-path bypasses
    // derive_review_verdict_artifact and uses meta.decision directly
    let mut meta =
        make_review_meta_with_decision(session_id, ReviewDecision::Unavailable, "UNAVAILABLE");
    meta.status_reason = Some("gemini_auth_prompt".to_string());
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Unavailable,
        "#1340: status_reason set means genuine failure — Unavailable must be preserved"
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1340 Case 4: unavailable meta still fails closed before findings promotion.
#[test]
fn issue_1340_high_finding_with_unavailable_meta_fails_closed() {
    let session_id = "01TEST1340HIGHFINDINGUNAVAIL";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1340-high-finding-unavailable-meta", session_id);

    let json_findings = vec![make_finding(Severity::High, "real-high")];
    let json_artifact = json!({
        "findings": json_findings,
        "severity_summary": SeveritySummary { critical: 0, high: 1, medium: 0, low: 0 },
        "overall_risk": "high"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&json_artifact).expect("serialize"),
    )
    .expect("write review-findings.json");

    let meta =
        make_review_meta_with_decision(session_id, ReviewDecision::Unavailable, "UNAVAILABLE");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Unavailable,
        "R2-001: HIGH finding with Unavailable meta must preserve unavailable"
    );
    assert_eq!(artifact.verdict_legacy, "UNAVAILABLE");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
