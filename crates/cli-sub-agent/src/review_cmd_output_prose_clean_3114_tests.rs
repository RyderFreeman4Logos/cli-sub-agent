use super::*;

#[test]
fn repairs_prose_unparsed_placeholder_when_review_is_clean() {
    let session_id = "01TEST3114PROSECLEAN000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-3114-prose-unparsed-clean", session_id);
    write_fail_meta(&session_dir, session_id);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nFindings: 0. Overall risk: Low. The prior HIGH finding is closed; no unresolved findings remain.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist clean review");
    write_empty_fail_placeholder_artifacts(&session_dir, session_id);
    let mut verdict = read_output_verdict(&session_dir);
    verdict.failure_reason = Some("prose_findings_present_but_unparsed".to_string());
    csa_session::write_review_verdict(&session_dir, &verdict).expect("write prose fail verdict");

    assert!(
        super::super::super::consistency::repair_clean_empty_fail_review_verdict(&session_dir)
            .expect("repair verdict"),
        "placeholder-only findings must not override conclusive clean prose"
    );

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert_eq!(verdict.failure_reason, None);
    assert!(verdict.severity_counts.values().all(|count| *count == 0));
    assert!(read_output_findings(&session_dir).findings.is_empty());

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn concrete_findings_dominate_prose_clean_summary() {
    let session_id = "01TESTFINDINGSDOMINATE000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-findings-dominate", session_id);
    let findings = vec![make_finding(Severity::High, "blocking-high")];
    let artifact = json!({
        "findings": findings,
        "severity_summary": SeveritySummary {
            critical: 0,
            high: 1,
            medium: 0,
            low: 0,
        },
        "overall_risk": "low"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("serialize findings"),
    )
    .expect("write findings artifact");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nNo blocking issues found.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist summary");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));
    let findings = read_output_findings(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].severity, Severity::High);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
