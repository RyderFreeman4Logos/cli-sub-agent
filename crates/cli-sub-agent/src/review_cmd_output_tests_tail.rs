#[test]
fn ensure_review_summary_artifact_writes_summary_md_when_section_emitted_without_prior_file() {
    let session_id = "01TESTSUMMARYWRITE0000000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("review-summary-write-from-output", session_id);
    let output = "<!-- CSA:SECTION:summary -->\nReview outcome: PASS.\n<!-- CSA:SECTION:summary:END -->\n\
<!-- CSA:SECTION:details -->\nDetails go here\n<!-- CSA:SECTION:details:END -->\n";

    ensure_review_summary_artifact(&session_dir, output).expect("write summary from output");

    let summary_path = session_dir.join("output").join("summary.md");
    assert!(summary_path.exists(), "summary.md must be written");
    let contents = fs::read_to_string(&summary_path).expect("read summary");
    assert!(
        contents.contains("Review outcome: PASS."),
        "got: {contents}"
    );

    let index = csa_session::load_output_index(&session_dir)
        .expect("load output index")
        .expect("index should exist");
    let summary_entry = index
        .sections
        .iter()
        .find(|section| section.id == "summary")
        .expect("summary entry should exist");
    assert_eq!(summary_entry.file_path.as_deref(), Some("summary.md"));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn ensure_review_summary_artifact_preserves_existing_summary_section() {
    let session_id = "01TESTSUMMARYKEEP0000000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("review-summary-preserve", session_id);
    let output = "<!-- CSA:SECTION:summary -->\nReview completed successfully.\n<!-- CSA:SECTION:summary:END -->\n\
<!-- CSA:SECTION:details -->\nDetailed body\n<!-- CSA:SECTION:details:END -->\n";
    csa_session::persist_structured_output(&session_dir, output).expect("persist structured");

    ensure_review_summary_artifact(&session_dir, output).expect("keep summary");

    let summary = csa_session::read_section(&session_dir, "summary")
        .expect("read summary section")
        .expect("summary should exist");
    assert_eq!(summary, "Review completed successfully.");

    let index = csa_session::load_output_index(&session_dir)
        .expect("load output index")
        .expect("index should exist");
    let summary_entries = index
        .sections
        .iter()
        .filter(|section| section.id == "summary")
        .count();
    assert_eq!(summary_entries, 1);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn ensure_review_summary_artifact_preserves_existing_multiline_summary_section() {
    let session_id = "01TESTSUMMARYMULTILINE000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("review-summary-preserve-multiline", session_id);
    let output = "<!-- CSA:SECTION:summary -->\nLine 1\nLine 2\nLine 3\n<!-- CSA:SECTION:summary:END -->\n\
<!-- CSA:SECTION:details -->\nFAIL\nDetailed body\n<!-- CSA:SECTION:details:END -->\n";
    csa_session::persist_structured_output(&session_dir, output).expect("persist structured");

    ensure_review_summary_artifact(&session_dir, output).expect("preserve multiline summary");

    let summary = csa_session::read_section(&session_dir, "summary")
        .expect("read summary section")
        .expect("summary should exist");
    assert_eq!(summary, "Line 1\nLine 2\nLine 3");

    let index = csa_session::load_output_index(&session_dir)
        .expect("load output index")
        .expect("index should exist");
    let summary_entries = index
        .sections
        .iter()
        .filter(|section| section.id == "summary")
        .count();
    assert_eq!(summary_entries, 1);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn ensure_review_summary_artifact_replaces_stale_summary_for_same_session_fix_rounds() {
    let session_id = "01TESTSUMMARYSAMEROUND000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("review-summary-same-session-fix", session_id);
    let round_1 = "<!-- CSA:SECTION:summary -->\nFAIL: stale finding\n<!-- CSA:SECTION:summary:END -->\n\
<!-- CSA:SECTION:details -->\nFAIL: stale finding\nDetailed body\n<!-- CSA:SECTION:details:END -->\n";
    csa_session::persist_structured_output(&session_dir, round_1).expect("persist round 1");
    ensure_review_summary_artifact(&session_dir, round_1).expect("persist round 1 summary");

    let round_2 = "<!-- CSA:SECTION:details -->\nCLEAN\nDetailed body after fix\n<!-- CSA:SECTION:details:END -->\n";
    csa_session::persist_structured_output(&session_dir, round_2).expect("persist round 2");
    ensure_review_summary_artifact(&session_dir, round_2).expect("refresh stale summary");

    let summary = csa_session::read_section(&session_dir, "summary")
        .expect("read summary section")
        .expect("summary should exist");
    assert_eq!(summary, "CLEAN");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_marks_clean_transcript_as_pass() {
    let session_id = "01TESTPASS0000000000000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-pass", session_id);
    let full_output = [json!({"type":"item.completed","item":{
        "id":"item_1",
        "type":"agent_message",
        "text":"<!-- CSA:SECTION:summary -->\nCLEAN\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nNo blocking issues found.\nOverall risk: low\n<!-- CSA:SECTION:details:END -->"
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
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_missing_new_bug_category_checklist_is_uncertain() {
    let session_id = "01TESTBUGCHECKLISTMISSING000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-missing-bug-checklist", session_id);
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_string_pretty(&json!({
            "findings": [],
            "severity_summary": {
                "critical": 0,
                "high": 0,
                "medium": 0,
                "low": 0
            },
            "review_mode": "standard",
            "schema_version": "1.1",
            "session_id": session_id,
            "timestamp": "2026-07-01T00:00:00Z",
            "overall_risk": "low"
        }))
        .expect("serialize review findings"),
    )
    .expect("write review-findings.json");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nCLEAN\n<!-- CSA:SECTION:summary:END -->\n\n\
<!-- CSA:SECTION:details -->\nNo blocking issues found.\nOverall risk: low\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist clean output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Uncertain);
    assert_eq!(artifact.verdict_legacy, "UNCERTAIN");
    assert_eq!(
        artifact.failure_reason.as_deref(),
        Some("missing_bug_category_checklist")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_plain_text_full_output_without_review_message_emits_fail_verdict() {
    let session_id = "01TESTMETAFALLBACK000000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-meta-fallback", session_id);
    fs::write(
        session_dir.join("output").join("full.md"),
        "Findings\n1. [High][regression] fallback path should preserve review_meta\nOverall risk: high",
    )
    .expect("write plain-text full output");

    let meta = make_review_meta(session_id);
    let findings = vec![make_finding(Severity::High, "fallback-high")];
    persist_review_verdict(&project_root, &meta, &findings, Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&1));
    assert_eq!(artifact.severity_counts.get(&Severity::Medium), Some(&0));
    assert_eq!(artifact.severity_counts.get(&Severity::Low), Some(&0));
    assert_eq!(artifact.severity_counts.get(&Severity::Critical), Some(&0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_concrete_findings_override_uncertain_token() {
    let session_id = "01TESTCONCRETEOVERUNCERTAIN";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-concrete-over-uncertain", session_id);
    let full_output = [json!({"type":"item.completed","item":{
        "id":"item_1",
        "type":"agent_message",
        "text":"<!-- CSA:SECTION:summary -->\nUNCERTAIN\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nNot-applicable to fuzzing, but there is still one concrete issue.\n1. [High][regression] parser disagreement remains user-visible.\nOverall risk: high\n<!-- CSA:SECTION:details:END -->"
    }})]
    .into_iter()
    .map(|line| serde_json::to_string(&line).expect("serialize transcript line"))
    .collect::<Vec<_>>()
    .join("\n");
    fs::write(session_dir.join("output").join("full.md"), full_output)
        .expect("write mixed verdict full output");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&1));
    assert_eq!(artifact.severity_counts.get(&Severity::Medium), Some(&0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_empty_structured_findings_uncertain_meta_fails_closed() {
    // R2-001: Uncertain reviewer outcome means the review did not complete with
    // positive pass evidence. Empty findings alone cannot synthesize Pass.
    let session_id = "01TESTEMPTYFINDINGSUNCERTAIN";
    let (_env_lock, project_root, session_dir) = lock_test_session(
        "persist-review-verdict-empty-findings-uncertain",
        session_id,
    );
    let findings_path = session_dir.join("review-findings.json");
    let artifact = json!({
        "findings": [],
        "severity_summary": {
            "critical": 0,
            "high": 0,
            "medium": 0,
            "low": 0,
            "info": 0
        },
        "overall_risk": "low"
    });
    fs::write(
        &findings_path,
        serde_json::to_vec_pretty(&artifact).expect("serialize findings"),
    )
    .expect("write findings artifact");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Uncertain, "UNCERTAIN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(
        artifact.decision,
        ReviewDecision::Uncertain,
        "R2-001: empty findings + zero counts must not promote Uncertain meta to Pass"
    );
    assert_eq!(artifact.verdict_legacy, "UNCERTAIN");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[path = "review_cmd_output_prose_clean_tests.rs"]
mod review_cmd_output_prose_clean_tests;

#[test]
fn persist_review_verdict_json_transcript_without_review_message_emits_uncertain_verdict() {
    let session_id = "01TESTJSONNOREVIEWMESSAGE00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-json-no-review-message", session_id);
    let full_output = [
        json!({"type":"thread.started","thread_id":"thread-1"}),
        json!({"type":"item.completed","item":{
            "id":"tool_1",
            "type":"tool_call",
            "name":"shell",
            "arguments":"echo checking"
        }}),
        json!({"type":"item.completed","item":{
            "id":"tool_2",
            "type":"tool_result",
            "output":"ok"
        }}),
    ]
    .into_iter()
    .map(|line| serde_json::to_string(&line).expect("serialize transcript line"))
    .collect::<Vec<_>>()
    .join("\n");
    fs::write(session_dir.join("output").join("full.md"), full_output)
        .expect("write tool-only full output transcript");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Uncertain, "UNCERTAIN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Uncertain);
    assert_eq!(artifact.verdict_legacy, "UNCERTAIN");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_json_noise_only_full_output_emits_uncertain_verdict() {
    let session_id = "01TESTJSONNOISEONLY00000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-json-noise-only", session_id);
    let full_output = [
        String::new(),
        "   ".to_string(),
        serde_json::to_string(&json!({"type":"thread.started","thread_id":"thread-1"}))
            .expect("serialize thread.started"),
        serde_json::to_string(&json!({"type":"thread.completed","thread_id":"thread-1"}))
            .expect("serialize thread.completed"),
        "\t".to_string(),
    ]
    .join("\n");
    fs::write(session_dir.join("output").join("full.md"), full_output)
        .expect("write json-noise full output transcript");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Uncertain, "UNCERTAIN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Uncertain);
    assert_eq!(artifact.verdict_legacy, "UNCERTAIN");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persisted_review_artifact_deserializes_optional_overall_risk() {
    let artifact: PersistedReviewArtifact = serde_json::from_value(json!({
        "findings": [],
        "severity_summary": {
            "critical": 0,
            "high": 0,
            "medium": 0,
            "low": 0,
            "info": 0
        },
        "overall_risk": "low"
    }))
    .expect("deserialize persisted review artifact");
    assert_eq!(artifact.overall_risk.as_deref(), Some("low"));
}

#[path = "review_cmd_output_verdict_1045_tests.rs"]
mod verdict_1045_tests;

#[path = "review_cmd_output_verdict_1340_tests.rs"]
mod verdict_1340_tests;

#[path = "review_cmd_output_verdict_1349_tests.rs"]
mod verdict_1349_tests;

#[path = "review_cmd_output_verdict_1352_tests.rs"]
mod verdict_1352_tests;

#[path = "review_cmd_output_verdict_1362_tests.rs"]
mod verdict_1362_tests;

#[path = "review_cmd_output_verdict_1480_tests.rs"]
mod verdict_1480_tests;

#[path = "review_cmd_output_verdict_1675_tests.rs"]
mod verdict_1675_tests;

#[path = "review_cmd_output_verdict_1716_tests.rs"]
mod verdict_1716_tests;

#[path = "review_cmd_output_verdict_1754_tests.rs"]
mod verdict_1754_tests;

#[path = "review_cmd_output_verdict_1761_tests.rs"]
mod verdict_1761_tests;

#[path = "review_cmd_output_verdict_1804_tests.rs"]
mod verdict_1804_tests;

#[path = "review_cmd_output_verdict_1852_tests.rs"]
mod verdict_1852_tests;
#[path = "review_cmd_output_verdict_1876_tests.rs"]
mod verdict_1876_tests;
