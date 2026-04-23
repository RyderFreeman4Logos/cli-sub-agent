use super::*;

/// Mirror of [`super::super::super::findings_toml::SYNTHETIC_MARKER_FILENAME`].
/// Duplicated here to avoid a 3-level super path from this nested test module.
const SYNTHETIC_MARKER_FILENAME: &str = ".findings.toml.synthetic";

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

/// Case 6 (#1045 round 2): synthetic-empty findings.toml (extraction failed) but
/// review-findings.json contains a HIGH finding → decision=fail.
///
/// This is the blind spot: the reviewer produced blocking findings in prose, but
/// the fenced TOML block was malformed/missing so findings.toml was written as
/// `findings = []`. The new cross-check must detect that review-findings.json
/// disagrees and emit fail.
#[test]
fn issue_1045_r2_synthetic_empty_toml_with_json_high_emits_fail() {
    let session_id = "01TEST1045R2SYNTHEMPTY00000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1045-r2-synthetic-empty", session_id);

    // Write synthetic-empty findings.toml (simulates failed TOML extraction).
    let findings_toml = "findings = []\n";
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        findings_toml,
    )
    .expect("write findings.toml");
    // Write sidecar synthetic marker (#1045 round 3 addition).
    fs::write(
        session_dir.join("output").join(SYNTHETIC_MARKER_FILENAME),
        b"",
    )
    .expect("write synthetic marker");

    // Write review-findings.json with a HIGH finding (reviewer's actual output).
    let json_findings = vec![make_finding(Severity::High, "real-high-finding")];
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

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Fail,
        "#1045 round 2: synthetic-empty findings.toml must not mask review-findings.json HIGH"
    );
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// Case 7 (#1045 round 2): true-empty findings.toml + empty review-findings.json → pass.
///
/// When both findings.toml and review-findings.json agree on zero findings,
/// the cross-check must not flip to fail. No synthetic marker present.
#[test]
fn issue_1045_r2_true_empty_toml_with_empty_json_emits_pass() {
    let session_id = "01TEST1045R2TRUEEMPTY000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1045-r2-true-empty", session_id);

    // True-empty findings.toml (reviewer genuinely found nothing — NO synthetic marker).
    let findings_toml = "findings = []\n";
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        findings_toml,
    )
    .expect("write findings.toml");

    // Empty review-findings.json (agrees: no findings).
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

    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\n**PASS** — No issues found.\n<!-- CSA:SECTION:summary:END -->\n",
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
        "#1045 round 2: true-empty + empty json must still pass"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

// ─── #1045 round 3 regression tests: synthetic-empty + missing JSON ──────

/// Case 8 (#1045 round 3): synthetic-empty findings.toml + NO review-findings.json
/// + structured [High] finding in full.md → decision=fail.
///
/// This is the round 3 bug: when both findings.toml is synthetic-empty AND
/// review-findings.json is absent, the code short-circuited with pass, silently
/// dropping blocking prose findings in full.md.
#[test]
fn issue_1045_r3_synthetic_empty_toml_missing_json_high_in_full_md_emits_fail() {
    let session_id = "01TEST1045R3SYNTHNOJS0HIGH0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1045-r3-synth-no-json-high-full", session_id);

    // Synthetic-empty findings.toml + sidecar marker.
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    fs::write(
        session_dir.join("output").join(SYNTHETIC_MARKER_FILENAME),
        b"",
    )
    .expect("write synthetic marker");

    // NO review-findings.json — deliberately absent.

    // full.md with structured [High] finding in transcript.
    let full_output = [json!({"type":"item.completed","item":{
        "id":"item_1",
        "type":"agent_message",
        "text":"<!-- CSA:SECTION:summary -->\nFAIL\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nFindings\n1. [High][correctness] derive_review_verdict_artifact short-circuits on synthetic-empty TOML, dropping full.md fallback.\n\nOverall risk: high\n<!-- CSA:SECTION:details:END -->"
    }})]
    .into_iter()
    .map(|line| serde_json::to_string(&line).expect("serialize transcript line"))
    .collect::<Vec<_>>()
    .join("\n");
    fs::write(session_dir.join("output").join("full.md"), full_output)
        .expect("write full output transcript");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Fail,
        "#1045 round 3: synthetic-empty TOML + missing JSON + [High] in full.md must fail"
    );
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// Case 9 (#1045 round 3): synthetic-empty findings.toml + NO review-findings.json
/// + non-empty unstructured full.md → decision=fail.
///
/// full.md has content but no severity markers — fail-closed via the
/// `!full_output_is_effectively_empty` fallback.
#[test]
fn issue_1045_r3_synthetic_empty_toml_missing_json_nonempty_unstructured_full_md_emits_fail() {
    let session_id = "01TEST1045R3SYNTHNOJSUNSTR0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1045-r3-synth-no-json-unstructured-full", session_id);

    // Synthetic-empty findings.toml + sidecar marker.
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    fs::write(
        session_dir.join("output").join(SYNTHETIC_MARKER_FILENAME),
        b"",
    )
    .expect("write synthetic marker");

    // NO review-findings.json.

    // full.md with unstructured (non-transcript) prose — no severity markers,
    // no CSA sections. Just plain text that isn't JSON and isn't empty.
    fs::write(
        session_dir.join("output").join("full.md"),
        "The reviewer produced some output but no structured verdict.\nSome notes about the diff.\n",
    )
    .expect("write unstructured full output");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Fail,
        "#1045 round 3: synthetic-empty TOML + missing JSON + non-empty unstructured full.md must fail-closed"
    );
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// Case 10 (#1045 round 3): synthetic-empty findings.toml + NO review-findings.json
/// + empty full.md → decision=uncertain.
///
/// Last-resort fallback when everything is empty/absent.
#[test]
fn issue_1045_r3_synthetic_empty_toml_missing_json_empty_full_md_emits_uncertain() {
    let session_id = "01TEST1045R3SYNTHNOJSEMPTY0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1045-r3-synth-no-json-empty-full", session_id);

    // Synthetic-empty findings.toml + sidecar marker.
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    fs::write(
        session_dir.join("output").join(SYNTHETIC_MARKER_FILENAME),
        b"",
    )
    .expect("write synthetic marker");

    // NO review-findings.json.
    // NO full.md (or empty — absent is equivalent for the empty check).

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Uncertain,
        "#1045 round 3: synthetic-empty TOML + missing JSON + empty/missing full.md must yield uncertain"
    );
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

// ─── #1048 MEDIUM regression tests ──────────────────────────────────────

/// #1048 M1: empty findings.toml + review-findings.json with ONLY low
/// findings → verdict must report severity_counts.low = 1 and decision = pass.
///
/// Bug: cross_check_json_for_blocking() returned None for low-only JSON,
/// so the caller rebuilt from zero TOML counts, dropping the low count.
#[test]
fn issue_1048_m1_low_only_json_preserves_severity_counts_low() {
    let session_id = "01TEST1048M1LOWONLYJSON0000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1048-m1-low-only-json", session_id);

    // True-empty findings.toml (no synthetic marker).
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");

    // review-findings.json with ONLY a low-severity finding.
    let json_findings = vec![make_finding(Severity::Low, "advisory-low")];
    let json_artifact = json!({
        "findings": json_findings,
        "severity_summary": SeveritySummary { critical: 0, high: 0, medium: 0, low: 1 },
        "overall_risk": "low"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&json_artifact).expect("serialize"),
    )
    .expect("write review-findings.json");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "#1048 M1: low-only JSON must not block"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert_eq!(
        artifact.severity_counts.get(&Severity::Low),
        Some(&1),
        "#1048 M1: low count from JSON must be preserved in verdict"
    );
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&0));
    assert_eq!(artifact.severity_counts.get(&Severity::Medium), Some(&0));
    assert_eq!(artifact.severity_counts.get(&Severity::Critical), Some(&0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1048 M2: full.md transcript with only [info]/[p4] advisory findings
/// → decision = pass, not fail.
///
/// Bug: derive_decision_from_text() treated any non-zero count as blocking.
#[test]
fn issue_1048_m2_info_only_full_md_transcript_emits_pass() {
    let session_id = "01TEST1048M2INFOONLYFULLMD0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1048-m2-info-only-full-md", session_id);

    // No findings.toml, no review-findings.json — only full.md.
    let full_output = [json!({"type":"item.completed","item":{
        "id":"item_1",
        "type":"agent_message",
        "text":"<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nFindings\n1. [Info][style] Minor formatting inconsistency.\n2. [P4][nit] Extra whitespace.\n\nNo blocking issues found in this scope.\nOverall risk: low\n<!-- CSA:SECTION:details:END -->"
    }})]
    .into_iter()
    .map(|line| serde_json::to_string(&line).expect("serialize transcript line"))
    .collect::<Vec<_>>()
    .join("\n");
    fs::write(session_dir.join("output").join("full.md"), full_output)
        .expect("write full output transcript");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "#1048 M2: [info]/[p4]-only transcript must not block"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert_eq!(
        artifact.severity_counts.get(&Severity::Low),
        Some(&2),
        "#1048 M2: two advisory findings should map to low count"
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
