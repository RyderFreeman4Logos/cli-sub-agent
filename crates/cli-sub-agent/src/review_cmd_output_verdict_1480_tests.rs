use super::*;

/// #1480: when the reviewer text contains "SKIP" but the structured
/// review-findings.json has zero findings and zero severity counts,
/// the zero-evidence Pass conclusion must win over the Skip noise from
/// meta_decision text-parse.
#[test]
fn issue_1480_skip_meta_with_zero_findings_emits_pass() {
    let decision = derive_decision_from_severity_counts(
        &BTreeMap::new(),
        true, // findings_empty
        None, // overall_risk
        Some(ReviewDecision::Skip),
        || Ok(false), // blocking summary signal absent
        || Ok(false), // prose_clean_check irrelevant: zero evidence fires first
        || Ok(false), // prose_fail_check irrelevant: Skip meta is not Fail/Uncertain
    )
    .expect("derive decision");

    assert_eq!(
        decision,
        ReviewDecision::Pass,
        "#1480: Skip meta_decision + zero findings + zero counts must yield Pass, not Skip"
    );
}

/// #1480: Skip meta + zero findings must also produce CLEAN verdict_legacy in
/// the full persist_review_verdict pipeline (not HAS_ISSUES from meta.verdict fallback).
#[test]
fn issue_1480_persist_verdict_skip_meta_zero_findings_json_emits_pass() {
    let session_id = "01TEST1480SKIPMETA0000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1480-skip-meta-zero-findings", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");

    // Reviewer wrote review-findings.json with zero findings — the structured
    // artifact is conclusively clean.
    let review_artifact = json!({
        "findings": [],
        "severity_summary": SeveritySummary { critical: 0, high: 0, medium: 0, low: 0 },
        "review_mode": "standard",
        "schema_version": "1.0",
        "session_id": session_id,
        "timestamp": chrono::Utc::now()
    });
    fs::write(
        session_dir.join(crate::bug_class::SINGLE_REVIEW_ARTIFACT_FILE),
        serde_json::to_vec_pretty(&review_artifact).expect("serialize review artifact"),
    )
    .expect("write review-findings.json");

    // meta.decision = "skip" / meta.verdict = "HAS_ISSUES": simulates the case
    // where the text parse erroneously picked up a SKIP token in the reviewer
    // output while the reviewer had already written its structured verdict as PASS.
    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Skip, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");

    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "#1480: zero-findings review-findings.json must override Skip meta_decision → Pass"
    );
    assert_eq!(
        artifact.verdict_legacy, "CLEAN",
        "#1480: verdict_legacy must be CLEAN, not HAS_ISSUES fallback from stale meta"
    );
    assert!(artifact.severity_counts.values().all(|c| *c == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// Regression guard: Skip meta with non-zero severity counts must fail closed —
/// only the zero-evidence case converts to Pass.
#[test]
fn issue_1480_skip_meta_with_nonzero_low_counts_fails_closed() {
    let mut counts = BTreeMap::new();
    counts.insert(Severity::Critical, 0u32);
    counts.insert(Severity::High, 0u32);
    counts.insert(Severity::Medium, 0u32);
    counts.insert(Severity::Low, 1u32);

    let decision = derive_decision_from_severity_counts(
        &counts,
        true, // findings_empty
        None, // overall_risk
        Some(ReviewDecision::Skip),
        || Ok(false),
        || Ok(false),
        || Ok(false),
    )
    .expect("derive decision");

    assert_eq!(
        decision,
        ReviewDecision::Fail,
        "Skip meta with non-zero counts must fail closed; only zero evidence converts to Pass"
    );
}
