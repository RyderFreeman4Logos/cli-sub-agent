use super::*;

// ─── #1352 regression tests: parse-error cascade must not manufacture Fail ───

/// #1352 Case 1: review-findings.json with severity="info" (valid per output-schema.md
/// but not in the Severity enum) must be silently ignored and yield Pass.
///
/// Root cause: `load_review_artifact_from_output` propagated serde parse errors as
/// `Err` via `?`. When the LLM wrote `severity = "info"`, serde_json failed to
/// deserialise the `Severity` enum → `Err` cascaded through
/// `cross_check_json_for_blocking` → `derive_review_verdict_artifact` returned `Err`
/// → `persist_review_verdict`'s error fallback copied `meta.decision = "fail"` directly
/// into the verdict JSON, bypassing the #1349 zero-counts guard entirely.
///
/// Fix: treat any parse error as "absent artifact" (Ok(None) + warn), so the
/// #1349 guard in `derive_decision_from_severity_counts` runs correctly.
#[test]
fn issue_1352_info_severity_in_json_treated_as_absent_emits_pass() {
    let session_id = "01TEST1352INFOSEVERITY00000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1352-info-severity-json", session_id);

    // Write review-findings.json with severity="info" — valid per output schema,
    // but `Severity` enum has no Info variant → triggers parse failure.
    fs::write(
        session_dir.join("review-findings.json"),
        r#"{"findings":[],"severity_summary":{"critical":0,"high":0,"medium":0,"low":0},"overall_risk":"low"}"#,
    )
    .expect("write review-findings.json");

    // Write a malformed entry with severity="info" to trigger the parse bug.
    fs::write(
        session_dir.join("review-findings.json"),
        r#"{"findings":[{"severity":"info","fid":"f1","file":"src/lib.rs","line":1,"rule_id":"rule.f1","summary":"informational note","engine":"reviewer"}],"severity_summary":{"critical":0,"high":0,"medium":0,"low":0},"overall_risk":"low"}"#,
    )
    .expect("write review-findings.json with info severity");

    // findings.toml also absent (triggers synthetic path → JSON cross-check).
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "#1352: info-severity parse error must not cascade to Fail; zero evidence → Pass"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|v| *v == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1352 Case 2: synthetic-empty findings.toml + review-findings.json has "info"
/// severity (parse error path). Should yield Pass.
///
/// Validates the primary real-world failure scenario: synthetic marker present
/// (TOML extraction failed) + JSON parse fails → persit_review_verdict error fallback.
#[test]
fn issue_1352_synthetic_findings_plus_info_json_emits_pass() {
    let session_id = "01TEST1352SYNTHINFOJSON0000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1352-synthetic-info-json", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    // Synthetic-empty findings.toml.
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    // Synthetic marker.
    fs::write(
        session_dir.join("output").join(".findings.toml.synthetic"),
        "",
    )
    .expect("write synthetic marker");
    // JSON with unrecognised severity — triggers parse failure.
    fs::write(
        session_dir.join("review-findings.json"),
        r#"{"findings":[{"severity":"info","fid":"f1","file":"src/lib.rs","line":1,"rule_id":"rule.f1","summary":"info note","engine":"reviewer"}],"severity_summary":{"critical":0,"high":0,"medium":0,"low":0},"overall_risk":"low"}"#,
    )
    .expect("write review-findings.json with info severity");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "#1352: synthetic-empty + info-severity JSON parse error must yield Pass"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1352 Case 3: derive_decision_from_text must NOT return Fail for FAIL/HAS_ISSUES
/// token when severity counts are all zero.
///
/// Bug: `derive_decision_from_text` had no zero-counts guard before the
/// FAIL/HAS_ISSUES token check. A neutral full.md with a "HAS_ISSUES" word anywhere
/// (e.g. in a diff description) would produce ReviewDecision::Fail with zero counts.
#[test]
fn issue_1352_derive_decision_from_text_zero_counts_ignores_fail_token() {
    let zero_counts = [
        (Severity::Critical, 0u32),
        (Severity::High, 0),
        (Severity::Medium, 0),
        (Severity::Low, 0),
    ]
    .into_iter()
    .collect::<std::collections::BTreeMap<_, _>>();

    let decision = derive_decision_from_text(
        "This diff HAS_ISSUES with the old approach but they are all fixed.\nOverall: CLEAN",
        &zero_counts,
        Some("low"),
    );
    assert_eq!(
        decision,
        ReviewDecision::Pass,
        "#1352: zero counts + FAIL token in neutral prose must not yield Fail"
    );
}

/// #1352 Case 4: derive_decision_from_text still returns Fail for HAS_ISSUES token
/// when non-zero blocking severity counts exist. Verify the fix is not over-broad.
#[test]
fn issue_1352_derive_decision_from_text_nonzero_counts_still_fails_on_token() {
    let counts = [
        (Severity::Critical, 0u32),
        (Severity::High, 1),
        (Severity::Medium, 0),
        (Severity::Low, 0),
    ]
    .into_iter()
    .collect::<std::collections::BTreeMap<_, _>>();

    let decision = derive_decision_from_text(
        "## VERDICT: HAS_ISSUES\nHigh-severity finding in auth module.",
        &counts,
        Some("high"),
    );
    assert_eq!(
        decision,
        ReviewDecision::Fail,
        "#1352: non-zero blocking counts + FAIL token must still yield Fail"
    );
}

/// #1352 Case 5: End-to-end — corrupt review-findings.json (invalid JSON) is treated
/// as absent and falls back to empty findings → Pass.
#[test]
fn issue_1352_corrupt_json_treated_as_absent_emits_pass() {
    let session_id = "01TEST1352CORRUPTJSON00000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1352-corrupt-json", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    fs::write(session_dir.join("review-findings.json"), b"not valid json")
        .expect("write corrupt review-findings.json");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "#1352: corrupt review-findings.json + empty findings.toml must yield Pass"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
