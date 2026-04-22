use super::*;

// ─── #1045 regression tests: verdict must derive from severity_counts ────

/// Case 1: Clean PASS summary + empty findings.toml → decision=pass.
/// Regression for issue #1045 where summary text parsing flipped decision to fail.
#[test]
fn issue_1045_clean_pass_summary_with_empty_findings_toml_emits_pass() {
    let session_id = "01TEST1045CLEANPASS000000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1045-clean-pass-findings-toml", session_id);
    let findings_toml = "findings = []\n";
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        findings_toml,
    )
    .expect("write findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\n**PASS** — Clean single-commit fix. No findings.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist summary");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "#1045 regression: zero findings must yield pass"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// Case 2: FAIL summary + HIGH finding → decision=fail.
#[test]
fn issue_1045_fail_summary_with_high_finding_emits_fail() {
    let session_id = "01TEST1045HIGHFINDING000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1045-high-finding", session_id);
    let findings = vec![make_finding(Severity::High, "blocking-high")];
    let artifact = json!({
        "findings": findings,
        "severity_summary": SeveritySummary { critical: 0, high: 1, medium: 0, low: 0 },
        "overall_risk": "high"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("serialize"),
    )
    .expect("write findings");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\n**FAIL** — 1 HIGH finding.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist summary");

    let meta = make_review_meta(session_id);
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

/// Case 3: PASS summary with word "fail" in explanatory prose + empty findings → decision=pass.
/// This is the core #1045 bug: "fail" in prose MUST NOT flip verdict when findings are empty.
#[test]
fn issue_1045_prose_containing_fail_keyword_with_empty_findings_emits_pass() {
    let session_id = "01TEST1045PROSEFAIL000000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1045-prose-fail-keyword", session_id);
    let findings_toml = "findings = []\n";
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        findings_toml,
    )
    .expect("write findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\n**PASS** but this reviewer would fail under different criteria. The approach is sound.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist summary");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "#1045 regression: 'fail' in prose must not flip verdict"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// Case 4: MEDIUM finding → decision=fail (MEDIUMs still count as findings).
#[test]
fn issue_1045_medium_finding_emits_fail() {
    let session_id = "01TEST1045MEDIUMFINDING00000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1045-medium-finding", session_id);
    let findings = vec![make_finding(Severity::Medium, "medium-issue")];
    let artifact = json!({
        "findings": findings,
        "severity_summary": SeveritySummary { critical: 0, high: 0, medium: 1, low: 0 },
        "overall_risk": "medium"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("serialize"),
    )
    .expect("write findings");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.severity_counts.get(&Severity::Medium), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// Case 5: LOW finding only → decision=pass (LOWs don't block merge).
#[test]
fn issue_1045_low_finding_only_emits_pass() {
    let session_id = "01TEST1045LOWFINDING0000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1045-low-finding", session_id);
    let findings = vec![make_finding(Severity::Low, "low-nit")];
    let artifact = json!({
        "findings": findings,
        "severity_summary": SeveritySummary { critical: 0, high: 0, medium: 0, low: 1 },
        "overall_risk": "low"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("serialize"),
    )
    .expect("write findings");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nNo blocking issues found.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist summary");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "LOW findings don't block"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert_eq!(artifact.severity_counts.get(&Severity::Low), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
