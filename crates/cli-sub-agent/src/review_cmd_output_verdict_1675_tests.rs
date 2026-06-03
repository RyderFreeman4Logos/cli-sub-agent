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

/// #1740: codex can emit prose findings in details.md as `N. High: ...` under
/// `## Findings`, while the structured findings artifact is empty and meta says
/// Pass/CLEAN. A blocking summary signal plus inline HIGH finding must fail.
#[test]
fn issue_1740_codex_inline_findings_and_blocking_summary_emit_fail() {
    let session_id = "01TEST1740CODEXPROSEHIGH00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1740-codex-prose-high", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nReview found one blocking contract issue in the debate failover trace path. ...\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\n## Findings\n\n1. High: `csa debate` can over-report `fallback_chain` for terminal non-success verdicts\n   File: crates/cli-sub-agent/src/debate_cmd.rs:467\n   ...\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Fail,
        "#1740: inline codex HIGH finding plus blocking summary must fail closed"
    );
    assert_ne!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert!(
        artifact
            .severity_counts
            .get(&Severity::High)
            .copied()
            .unwrap_or_default()
            >= 1,
        "#1740: inline `High:` finding must be reflected in severity_counts"
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1740 round 2 + #1806: "non-blocking issue" is not a blocking summary,
/// but any parsed `## Findings` entry still fails closed.
#[test]
fn issue_1740_non_blocking_issue_summary_with_low_only_finding_fails_closed() {
    let session_id = "01TEST1740NONBLOCKINGLOW00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1740-non-blocking-low", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nFound one non-blocking issue.\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\n## Findings\n\n1. Low: Minor wording nit in the review summary\n   File: crates/cli-sub-agent/src/review_cmd_output.rs:1\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist structured output");

    assert!(
        !contains_blocking_issue_signal("Found one non-blocking issue."),
        "#1740 round 2: non-blocking issue must not count as a blocking summary"
    );
    assert!(!contains_blocking_issue_signal(
        "Found one non blocking issue."
    ));
    assert!(!contains_blocking_issue_signal(
        "Found one nonblocking issue."
    ));
    assert!(contains_blocking_issue_signal("Found one blocking issue."));

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Fail,
        "#1806: any parsed finding in a Findings section must fail closed"
    );
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(
        artifact.severity_counts.get(&Severity::Low),
        Some(&1),
        "#1740 round 2: inline `Low:` finding must be reflected in severity_counts"
    );

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

/// #1675 Case 4 (review-finding follow-up): an affirmative FAIL verdict that lives
/// ONLY in the `details` section — neutral `summary`, no `output/full.md` — MUST
/// still fail closed. The initial fix scanned only `summary` + `full.md`, so a
/// structured review that records its verdict solely in `details` would slip
/// through as a false Pass (flagged by the heterogeneous codex review of this fix).
#[test]
fn issue_1675_fail_verdict_only_in_details_section_emits_fail() {
    let session_id = "01TEST1675DETAILSFAILONLY00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1675-details-fail-only", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    // Neutral summary, FAIL verdict only in details, and no output/full.md: the
    // summary-only scan would miss this and false-Pass.
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nReviewed the diff.\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\nVerdict: FAIL\n\nTwo blocking issues found.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Fail,
        "#1675: affirmative FAIL verdict only in details section must fail closed"
    );
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1675 Case 5 (review-finding follow-up): a FAIL verdict in a DUPLICATE later
/// `details` section must fail closed. `read_section` returns only the first
/// section per id, so an early neutral `details` followed by a later `details`
/// holding the real FAIL verdict (persisted as `details-2.md`) could hide it.
/// The fail-closed scan uses `read_all_sections`, so every duplicate is checked.
#[test]
fn issue_1675_fail_verdict_in_duplicate_later_details_section_emits_fail() {
    let session_id = "01TEST1675DUPDETAILSFAIL000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1675-dup-details-fail", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    // Neutral summary + neutral FIRST details; the FAIL verdict lives only in the
    // SECOND (duplicate) details section (persisted as details-2.md) and there is
    // no full.md. read_section's first-match would miss it; read_all_sections sees it.
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nReviewed.\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\nLooks fine on first pass.\n<!-- CSA:SECTION:details:END -->\n<!-- CSA:SECTION:details -->\nVerdict: FAIL\n\nBlocking issue on closer inspection.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Fail,
        "#1675: FAIL verdict in a duplicate later details section must fail closed"
    );
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1675 Case 6 (review-finding follow-up): a MIXED-CASE affirmative FAIL verdict
/// (`Verdict: Fail`) must fail closed. The CLI's verdict parser matches tokens
/// case-insensitively (`eq_ignore_ascii_case`), so `Verdict: Fail` sets meta=Fail;
/// the fail-closed prose detector was case-sensitive and missed it, reopening the
/// #1675 lost-evidence path for case variants. The detector is now case-insensitive.
#[test]
fn issue_1675_mixed_case_fail_verdict_emits_fail() {
    let session_id = "01TEST1675MIXEDCASEFAIL0000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1675-mixed-case-fail", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    // Mixed-case "Verdict: Fail": recognized by the CLI's case-insensitive verdict
    // parser (meta=Fail) but missed by the old case-sensitive prose detector.
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nVerdict: Fail\n\nOne blocking issue.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Fail,
        "#1675: mixed-case 'Verdict: Fail' must fail closed (case-insensitive detector)"
    );
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1675 Case 7 (review-finding follow-up): a bare colon-terminated FAIL verdict
/// (`FAIL:`) must fail closed. The CLI's consensus parser splits on
/// non-alphanumeric/non-`_` delimiters, so `FAIL:` counts as a blocking verdict
/// (meta=Fail); the bare-line prose detector trimmed only `{ws,*,_,.}` and missed
/// the trailing colon, reopening the #1675 lost-evidence path. The detector now
/// trims the full consensus delimiter class.
#[test]
fn issue_1675_colon_terminated_fail_verdict_emits_fail() {
    let session_id = "01TEST1675COLONFAIL00000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1675-colon-fail", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    // Bare colon-terminated FAIL verdict: counted by the consensus parser but
    // missed by the old bare-line detector that did not trim the trailing `:`.
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nFAIL:\n\nThe build is broken.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Fail,
        "#1675: bare colon-terminated 'FAIL:' must fail closed"
    );
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1675 Case 8 (cloud-review P1 follow-up): the SYNTHETIC-empty findings.toml
/// path had the SAME zero-evidence false-PASS hole. When findings.toml fails to
/// parse, `persist_review_findings_toml` writes a `.findings.toml.synthetic`
/// marker (#1045 r3) and the verdict derivation falls through the artifact chain.
/// If no blocking JSON, no JSON artifact, and no `full.md` exist, the pre-fix code
/// returned Pass UNCONDITIONALLY at the synthetic fallback — reopening #1675 on the
/// synthetic branch. The synthetic fallback now routes through the shared
/// `derive_decision_from_severity_counts` gate, so an affirmative prose FAIL with a
/// Fail meta fails closed even when the structured findings were unparseable.
#[test]
fn issue_1675_synthetic_empty_findings_with_prose_fail_emits_fail() {
    let session_id = "01TEST1675SYNTHETICFAIL0000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1675-synthetic-fail", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    // Synthetic marker: findings.toml extraction failed, so the empty findings are
    // NOT trusted and the verdict falls through the artifact chain (#1045 r3).
    fs::write(
        session_dir
            .join("output")
            .join(crate::review_cmd::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER),
        "",
    )
    .expect("write synthetic marker");
    // Affirmative prose FAIL in summary; no output/full.md so the synthetic
    // fallback (not infer_review_verdict_from_full_output) decides the verdict.
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nVerdict: FAIL\n\nBlocking issue; structured findings failed to emit.\n<!-- CSA:SECTION:summary:END -->\n",
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
        "#1675 P1: synthetic-empty findings + affirmative prose FAIL must fail closed"
    );
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1675 Case 9 (cloud-review P1 follow-up, precision guard): routing the synthetic
/// fallback through the shared gate must NOT over-correct. Synthetic-empty findings
/// with NEUTRAL prose (no FAIL verdict token) and no full.md must still resolve to
/// Pass — preserving the #1045-r3 / #1349 behavior on the synthetic branch.
#[test]
fn issue_1675_synthetic_empty_findings_with_neutral_prose_stays_pass() {
    let session_id = "01TEST1675SYNTHETICPASS0000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1675-synthetic-pass", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    fs::write(
        session_dir
            .join("output")
            .join(crate::review_cmd::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER),
        "",
    )
    .expect("write synthetic marker");
    // Neutral prose, no FAIL verdict token: the synthetic fallback must stay Pass.
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nSemantics-preserving refactor; findings block was malformed.\n<!-- CSA:SECTION:summary:END -->\n",
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
        "#1675 P1: synthetic-empty + neutral prose must stay Pass (no over-correction)"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

/// #1675 Case 10 (cloud-review round-7 follow-up): a FAIL verdict that survives ONLY
/// in the raw `output.log` (full.md absent, sections neutral, findings.toml
/// synthetic-empty) must fail closed. The findings extractor reads `output.log` when
/// `full.md` is missing, but the fail-closed detector previously stopped at full.md —
/// so the synthetic fallback false-passed. Detector and extractor now share
/// `load_canonical_review_text`, so their source sets match.
#[test]
fn issue_1675_synthetic_empty_fail_verdict_only_in_output_log_emits_fail() {
    let session_id = "01TEST1675OUTPUTLOGFAIL0000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1675-output-log-fail", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    fs::write(
        session_dir
            .join("output")
            .join(crate::review_cmd::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER),
        "",
    )
    .expect("write synthetic marker");
    // Neutral summary + details sections (step-1 section scan finds no FAIL) and NO
    // full.md, so load_canonical_review_text falls back to output.log.
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nReviewed the diff.\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\nNotes on the change.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist structured output");
    // The affirmative FAIL verdict survives only in the raw transcript (output.log)
    // as a reviewer agent message, outside the persisted sections.
    fs::write(
        session_dir.join("output.log"),
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"Verdict: FAIL"}}"#,
    )
    .expect("write output.log");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Fail,
        "#1675 r7: FAIL verdict only in output.log must fail closed (shared extractor source set)"
    );
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");

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

#[test]
fn detect_prose_fail_conclusion_mixed_case_labeled_is_true() {
    // The CLI verdict parser matches case-insensitively; the detector must agree.
    assert!(detect_prose_fail_conclusion("Verdict: Fail"));
    assert!(detect_prose_fail_conclusion("verdict: fail"));
}

#[test]
fn detect_prose_fail_conclusion_lowercase_bare_token_is_true() {
    assert!(detect_prose_fail_conclusion(
        "review notes\nfail\nmore notes"
    ));
}

#[test]
fn detect_prose_fail_conclusion_mixed_case_emphasized_is_true() {
    assert!(detect_prose_fail_conclusion("**Fail**"));
}

#[test]
fn detect_prose_fail_conclusion_lowercase_benign_substring_is_false() {
    // Case-insensitivity must NOT regress precision: "fails" is still not a token.
    assert!(!detect_prose_fail_conclusion(
        "the run fails intermittently"
    ));
}

#[test]
fn detect_prose_fail_conclusion_colon_terminated_is_true() {
    // codex finding: a bare verdict token followed by a consensus delimiter (`:`)
    // must be recognized — the consensus parser counts it as a blocking verdict.
    assert!(detect_prose_fail_conclusion("FAIL:"));
    assert!(detect_prose_fail_conclusion("review\nHAS_ISSUES:\nmore"));
}
