use super::*;

fn write_empty_fail_placeholder_artifacts(session_dir: &Path, session_id: &str) {
    let mut verdict = ReviewVerdictArtifact::from_parts(
        session_id.to_string(),
        ReviewDecision::Fail,
        "HAS_ISSUES",
        &[],
        Vec::new(),
    );
    verdict.failure_reason = Some("fail_verdict_empty_findings_artifact".to_string());
    verdict.severity_counts.insert(Severity::Medium, 1);
    csa_session::write_review_verdict(session_dir, &verdict).expect("write fail verdict");
    csa_session::write_findings_toml(
        session_dir,
        &csa_session::FindingsFile {
            findings: vec![csa_session::ReviewFinding {
                id: "artifact-generation-001".to_string(),
                severity: Severity::Medium,
                file_ranges: Vec::new(),
                is_regression_of_commit: None,
                suggested_test_scenario: None,
                description: "Artifact generation failed: review verdict is FAIL but CSA could not extract a structured finding. Reason: fail_verdict_empty_findings_artifact. Inspect output/details.md and output/review-verdict.json.".to_string(),
            }],
        },
    )
    .expect("write placeholder findings.toml");
}

fn write_fail_meta(session_dir: &Path, session_id: &str) {
    csa_session::write_review_meta(session_dir, &make_review_meta(session_id))
        .expect("write review meta");
}

fn read_output_verdict(session_dir: &Path) -> ReviewVerdictArtifact {
    serde_json::from_str(
        &fs::read_to_string(session_dir.join("output").join("review-verdict.json"))
            .expect("read verdict"),
    )
    .expect("parse verdict")
}

fn read_output_findings(session_dir: &Path) -> csa_session::FindingsFile {
    toml::from_str(
        &fs::read_to_string(session_dir.join("output").join("findings.toml"))
            .expect("read findings.toml"),
    )
    .expect("parse findings.toml")
}

fn write_agent_message_output_log(session_dir: &Path, messages: &[&str]) {
    let lines = messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            serde_json::to_string(&json!({
                "type": "item.completed",
                "item": {
                    "id": format!("item_{index}"),
                    "type": "agent_message",
                    "text": message,
                }
            }))
            .expect("serialize transcript event")
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(session_dir.join("output.log"), lines).expect("write output.log");
}

#[test]
fn issue_2405_repairs_empty_fail_artifact_when_summary_is_pass() {
    let session_id = "01TEST2405PASSSUMMARY0000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2405-pass-summary", session_id);
    write_fail_meta(&session_dir, session_id);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nNo blocking findings.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist clean sections");
    write_empty_fail_placeholder_artifacts(&session_dir, session_id);

    assert!(
        super::super::consistency::repair_clean_empty_fail_review_verdict(&session_dir)
            .expect("repair verdict"),
        "clean PASS summary should repair the empty-fail placeholder"
    );

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert_eq!(verdict.failure_reason, None);
    assert!(verdict.severity_counts.values().all(|count| *count == 0));
    let findings = read_output_findings(&session_dir);
    assert!(findings.findings.is_empty());

    let meta = fs::read_to_string(session_dir.join("review_meta.json")).expect("read meta");
    let meta: ReviewSessionMeta = serde_json::from_str(&meta).expect("parse meta");
    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(meta.verdict, "CLEAN");
    assert_eq!(meta.exit_code, 0);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2405_repairs_empty_fail_artifact_when_summary_has_no_blockers() {
    let session_id = "01TEST2405NOBLOCKERS00000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2405-no-blockers-summary", session_id);
    write_fail_meta(&session_dir, session_id);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `main...HEAD`. The prior high finding is resolved.\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nOpen questions:\n- None blocking.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist clean sections");
    write_empty_fail_placeholder_artifacts(&session_dir, session_id);

    assert!(
        super::super::consistency::repair_clean_empty_fail_review_verdict(&session_dir)
            .expect("repair verdict"),
        "no-blocker summary should repair the empty-fail placeholder"
    );

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    let findings = read_output_findings(&session_dir);
    assert!(findings.findings.is_empty());

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2405_output_log_stale_fail_then_clean_repairs_empty_fail_artifact() {
    let session_id = "01TEST2405OUTPUTLOGCLEAN00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2405-output-log-stale-fail", session_id);
    write_fail_meta(&session_dir, session_id);
    let prior_fail = concat!(
        "<!-- CSA:SECTION:summary -->\n",
        "Verdict: FAIL\n",
        "<!-- CSA:SECTION:summary:END -->\n\n",
        "<!-- CSA:SECTION:details -->\n",
        "## Findings\n\n",
        "1. High: stale pre-fix finding in src/lib.rs:1.\n",
        "<!-- CSA:SECTION:details:END -->\n",
    );
    let final_clean = concat!(
        "<!-- CSA:SECTION:summary -->\n",
        "PASS\n",
        "<!-- CSA:SECTION:summary:END -->\n\n",
        "<!-- CSA:SECTION:details -->\n",
        "No blocking findings remain.\n",
        "<!-- CSA:SECTION:details:END -->\n",
    );
    write_agent_message_output_log(&session_dir, &[prior_fail, final_clean]);
    csa_session::persist_structured_output_from_file(&session_dir, &session_dir.join("output.log"))
        .expect("refresh structured output from output.log");
    write_empty_fail_placeholder_artifacts(&session_dir, session_id);

    assert!(
        super::super::consistency::repair_clean_empty_fail_review_verdict(&session_dir)
            .expect("repair verdict"),
        "current clean round from output.log should repair despite stale historical fail text"
    );

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert_eq!(verdict.failure_reason, None);
    assert!(verdict.severity_counts.values().all(|count| *count == 0));
    let findings = read_output_findings(&session_dir);
    assert!(findings.findings.is_empty());

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2405_mixed_pass_fail_empty_artifact_stays_fail_closed() {
    let session_id = "01TEST2405MIXEDFAIL00000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2405-mixed-fail", session_id);
    write_fail_meta(&session_dir, session_id);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nReview verdict: FAIL. One blocking correctness issue remains.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist mixed sections");
    write_empty_fail_placeholder_artifacts(&session_dir, session_id);

    assert!(
        !super::super::consistency::repair_clean_empty_fail_review_verdict(&session_dir)
            .expect("repair verdict"),
        "mixed PASS/FAIL text must not repair to clean"
    );

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(
        verdict.failure_reason.as_deref(),
        Some("fail_verdict_empty_findings_artifact")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2405_duplicate_stale_clean_summary_does_not_repair_to_pass() {
    let session_id = "01TEST2405STALECLEAN0000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2405-stale-clean-summary", session_id);
    write_fail_meta(&session_dir, session_id);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:summary -->\nReview incomplete; final decision withheld.\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nMore evidence is needed before accepting this review.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist duplicate clean then neutral sections");
    write_empty_fail_placeholder_artifacts(&session_dir, session_id);

    assert!(
        !super::super::consistency::repair_clean_empty_fail_review_verdict(&session_dir)
            .expect("repair verdict"),
        "a stale clean duplicate must not repair the current neutral review to clean"
    );

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(
        verdict.failure_reason.as_deref(),
        Some("fail_verdict_empty_findings_artifact")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2405_stale_clean_details_do_not_repair_later_neutral_summary() {
    let session_id = "01TEST2405STALEDETAILS000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2405-stale-clean-details", session_id);
    write_fail_meta(&session_dir, session_id);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nNo blocking findings.\n<!-- CSA:SECTION:details:END -->\n\n<!-- CSA:SECTION:summary -->\nReview incomplete; final decision withheld.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist stale clean details then neutral summary");
    write_empty_fail_placeholder_artifacts(&session_dir, session_id);

    assert!(
        !super::super::consistency::repair_clean_empty_fail_review_verdict(&session_dir)
            .expect("repair verdict"),
        "stale clean details must not repair a later neutral summary-only review to clean"
    );

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(
        verdict.failure_reason.as_deref(),
        Some("fail_verdict_empty_findings_artifact")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

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
fn persist_review_verdict_fail_meta_without_prose_clean_marker_emits_pass() {
    // #1349: empty findings + zero counts is conclusive Pass, regardless of prose.
    // The prior #1144 policy (preserve Fail unless prose says CLEAN) is superseded:
    // structured evidence (empty findings.toml / review-findings.json with zero counts)
    // is authoritative. meta_decision=Fail from exit-code or text-parse heuristics
    // must not override zero-evidence structured records.
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
    assert_eq!(
        artifact.decision,
        ReviewDecision::Pass,
        "#1349: empty findings + zero counts must yield Pass even without clean prose"
    );
    assert_eq!(artifact.verdict_legacy, "CLEAN");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_uncertain_meta_with_pass_prose_fails_closed_post_crash() {
    // R2-001: when reviewer metadata says Uncertain, the reviewer did not
    // complete with a trustworthy pass outcome. Even explicit PASS prose cannot
    // override the incomplete review outcome.
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
    meta.failure_reason = Some("reviewer process crashed before artifact finalization".to_string());
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Uncertain,
        "R2-001: Uncertain meta must fail closed despite PASS prose"
    );
    assert_eq!(artifact.verdict_legacy, "UNCERTAIN");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_uncertain_meta_without_pass_prose_fails_closed() {
    // R2-001: empty findings are not positive proof of success when the review
    // outcome is Uncertain.
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
        "R2-001: Uncertain meta + empty findings must fail closed"
    );
    assert_eq!(artifact.verdict_legacy, "UNCERTAIN");

    fs::remove_dir_all(project_root).expect("remove temp project root");
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
    assert_eq!(artifact.severity_counts.get(&Severity::Medium), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[path = "review_cmd_output_prose_clean_2425_tests.rs"]
mod issue_2425;
