use super::*;

#[test]
fn persist_review_verdict_empty_findings_with_prose_clean_summary_emits_pass() {
    let session_id = "01TESTPROSECLEANEN000000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-prose-clean-en", session_id);
    let artifact = json!({
        "findings": [],
        "severity_summary": SeveritySummary::default(),
        "overall_risk": "low"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("serialize findings"),
    )
    .expect("write findings artifact");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nNo blocking correctness, contract, or security issues found.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist summary");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_empty_findings_with_chinese_prose_clean_summary_emits_pass() {
    let session_id = "01TESTPROSECLEANCN000000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-prose-clean-cn", session_id);
    let artifact = json!({
        "findings": [],
        "severity_summary": SeveritySummary::default(),
        "overall_risk": "low"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("serialize findings"),
    )
    .expect("write findings artifact");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\n\u{672a}\u{53d1}\u{73b0}\u{9700}\u{8981}\u{963b}\u{585e}\u{5408}\u{5e76}\u{7684}\u{95ee}\u{9898}\u{ff0c}\u{5c5e}\u{4e8e}\u{8bed}\u{4e49}\u{7b49}\u{4ef7}\u{91cd}\u{6784}\u{3002}\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist summary");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_empty_findings_without_prose_clean_marker_emits_pass() {
    // #1045: zero findings + zero severity_counts MUST yield Pass regardless
    // of whether the summary contains a prose-clean phrase.
    let session_id = "01TESTNOPROSECLEAN000000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-no-prose-clean", session_id);
    let artifact = json!({
        "findings": [],
        "severity_summary": SeveritySummary::default(),
        "overall_risk": "low"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("serialize findings"),
    )
    .expect("write findings artifact");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nSemantics-preserving refactor.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist summary");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "CLEAN");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_uncertain_meta_with_pass_prose_emits_pass_post_crash() {
    // #1140: when an upstream crash (e.g. claude-code reviewer-1 EROFS writing
    // /home/obj/.claude.json after the structured verdict has already been
    // emitted) flips meta.decision to Uncertain, the synthesizer must trust
    // the existing severity_counts (all zero) + findings.toml (empty) +
    // explicit "Verdict: PASS" prose to recover the real verdict. Previously
    // any Uncertain meta short-circuited the synthesizer and propagated as
    // Uncertain regardless of the unambiguous structured signals.
    let session_id = "01TESTUNCERTAINPASSCRASH000";
    let (_env_lock, project_root, session_dir) = lock_test_session(
        "persist-review-verdict-uncertain-meta-pass-prose",
        session_id,
    );
    let artifact = json!({
        "findings": [],
        "severity_summary": SeveritySummary::default(),
        "overall_risk": "low"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("serialize findings"),
    )
    .expect("write findings artifact");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nVerdict: PASS. The commit aligns the default timeout with the policy across tests, PATTERN.md, and workflow.toml.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist summary");

    let mut meta = make_review_meta(session_id);
    meta.decision = ReviewDecision::Uncertain.as_str().to_string();
    meta.verdict = "UNCERTAIN".to_string();
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "Uncertain meta + zero severity + empty findings + 'Verdict: PASS' prose must downgrade to Pass"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_uncertain_meta_without_pass_prose_preserves_uncertain() {
    // #1140 inverse: when meta.decision is Uncertain AND prose offers no
    // PASS/CLEAN signal, the synthesizer must NOT silently downgrade to Pass.
    // Preserves the deliberate-uncertainty path for the caller to handle.
    let session_id = "01TESTUNCERTAINNOSIGNAL0000";
    let (_env_lock, project_root, session_dir) = lock_test_session(
        "persist-review-verdict-uncertain-meta-no-signal",
        session_id,
    );
    let artifact = json!({
        "findings": [],
        "severity_summary": SeveritySummary::default(),
        "overall_risk": "low"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("serialize findings"),
    )
    .expect("write findings artifact");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nReview ambiguous; insufficient signal to classify.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist summary");

    let mut meta = make_review_meta(session_id);
    meta.decision = ReviewDecision::Uncertain.as_str().to_string();
    meta.verdict = "UNCERTAIN".to_string();
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Uncertain,
        "Uncertain meta with no PASS/CLEAN prose must remain Uncertain (no silent merge)"
    );
}

#[test]
fn persist_review_verdict_findings_dominate_prose_clean_summary() {
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

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_prose_clean_summary_respects_high_overall_risk_fail_closed() {
    let session_id = "01TESTPROSECLEANRISK0000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-prose-clean-high-risk", session_id);
    let artifact = json!({
        "findings": [],
        "severity_summary": SeveritySummary::default(),
        "overall_risk": "high"
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

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
