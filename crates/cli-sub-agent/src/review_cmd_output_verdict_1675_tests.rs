use super::*;

// ─── #1675 regression tests: fail-closed on affirmative prose FAIL with empty findings ─
//
// Bug: `csa review` produced a prose verdict of FAIL with blocking findings in
// summary.md/details.md, but the structured artifacts were empty (findings.toml =
// `findings = []`, zero severity counts). `derive_decision_from_severity_counts`
// unconditionally returned Pass for empty findings + zero counts, dropping the
// genuine FAIL and silently merging blocking findings.
//
// Fix (#1675): when meta_decision is Fail/Uncertain AND the prose AFFIRMATIVELY
// concludes FAIL, fail closed. The discriminator vs #1349 (which must stay Pass)
// is an affirmative prose FAIL verdict token — NOT "prose is not clean".

/// #1675 Case 1: meta=Fail + empty findings.toml + summary with an affirmative
/// FAIL verdict token → decision MUST be Fail (lost-evidence fail-closed).
#[test]
fn issue_1675_fail_meta_empty_findings_with_prose_fail_verdict_emits_fail() {
    let session_id = "01TEST1675PROSEFAILVERDICT0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1675-prose-fail-verdict", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    // Reviewer prose affirmatively concludes FAIL, but the structured findings
    // failed to emit (findings.toml is empty) — the #1675 lost-evidence scenario.
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nVerdict: FAIL\n\nTwo blocking issues found in the diff.\n<!-- CSA:SECTION:summary:END -->\n",
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
        ReviewDecision::Fail,
        "#1675: Fail meta + empty findings + affirmative prose FAIL must fail closed"
    );
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1675 Case 2: regression guard for #1349 — meta=Fail + empty findings +
/// NEUTRAL prose (no FAIL verdict token) MUST stay Pass.
#[test]
fn issue_1675_does_not_regress_1349_neutral_prose_stays_pass() {
    let session_id = "01TEST1675NEUTRALPROSE00000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1675-neutral-prose", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    // Neutral prose — no FAIL verdict token. This is the #1349 noise case
    // (meta=Fail from exit-code/quota-fallback while the reviewer found nothing).
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
        "#1675: neutral prose (no FAIL token) must NOT regress #1349 — stays Pass"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1675 Case 3 (precision): meta=Fail + empty findings + prose that mentions the
/// substring "fail" benignly (no verdict token) MUST stay Pass. Detecting the
/// substring would re-introduce false-FAIL ping-pong.
#[test]
fn issue_1675_benign_fail_mention_in_clean_prose_stays_pass() {
    let session_id = "01TEST1675BENIGNFAILMENTION";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1675-benign-fail-mention", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    // Substring "fail" appears ("failing", "fails") but there is no verdict token.
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nAll tests pass; the previously failing case no longer fails.\n<!-- CSA:SECTION:summary:END -->\n",
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
        "#1675: benign 'fail' substring (no verdict token) must stay Pass (precision)"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

// ─── Unit tests for detect_prose_fail_conclusion ───

#[test]
fn detect_prose_fail_conclusion_labeled_verdict_is_true() {
    assert!(detect_prose_fail_conclusion("Verdict: FAIL"));
}

#[test]
fn detect_prose_fail_conclusion_emphasized_fail_is_true() {
    assert!(detect_prose_fail_conclusion("**FAIL**"));
}

#[test]
fn detect_prose_fail_conclusion_bare_has_issues_line_is_true() {
    assert!(detect_prose_fail_conclusion("details\nHAS_ISSUES\nnotes"));
}

#[test]
fn detect_prose_fail_conclusion_benign_fail_substring_is_false() {
    assert!(!detect_prose_fail_conclusion("the build no longer fails"));
}

#[test]
fn detect_prose_fail_conclusion_pass_verdict_is_false() {
    assert!(!detect_prose_fail_conclusion("Verdict: PASS"));
}
