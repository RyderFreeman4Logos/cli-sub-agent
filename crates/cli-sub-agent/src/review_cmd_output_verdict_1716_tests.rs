use super::*;

#[test]
fn issue_1716_failed_final_reviewer_with_synthetic_empty_findings_is_unavailable() {
    let session_id = "01TEST1716FAILEDREVIEWER000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1716-failed-reviewer", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    fs::write(
        session_dir.join("output").join(".findings.toml.synthetic"),
        "",
    )
    .expect("write synthetic marker");
    fs::write(
        session_dir.join("output").join("full.md"),
        "I'll read the workflow, then check the diff before writing findings.\n",
    )
    .expect("write setup-only output");

    let mut meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    meta.exit_code = 137;
    meta.primary_failure = Some("API key not found".to_string());
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Unavailable);
    assert_eq!(artifact.verdict_legacy, "UNAVAILABLE");
    assert_eq!(
        artifact.primary_failure.as_deref(),
        Some("API key not found")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1716_unavailable_nonzero_empty_failure_metadata_is_unavailable() {
    let session_id = "01TEST1716UNAVAILABLEEMPTY00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1716-unavailable-empty-meta", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write findings.toml");
    fs::write(
        session_dir.join("output").join(".findings.toml.synthetic"),
        "",
    )
    .expect("write synthetic marker");
    fs::write(
        session_dir.join("output").join("full.md"),
        "reviewer infrastructure became unavailable before producing a verdict\n",
    )
    .expect("write setup-only output");

    let mut meta =
        make_review_meta_with_decision(session_id, ReviewDecision::Unavailable, "UNAVAILABLE");
    meta.exit_code = 1;
    meta.status_reason = None;
    meta.primary_failure = None;
    meta.failure_reason = None;
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Unavailable);
    assert_eq!(artifact.verdict_legacy, "UNAVAILABLE");
    assert_ne!(artifact.decision, ReviewDecision::Pass);
    assert_ne!(artifact.verdict_legacy, "CLEAN");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1930_terminal_tool_error_after_pass_text_persists_unavailable() {
    let session_id = "01TEST1930ISERRORPASS0000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1930-terminal-error-pass", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("full.md"),
        [
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\nNo blocking issues found.\n<!-- CSA:SECTION:details:END -->"}}"#,
            r#"{"type":"result","subtype":"error_api","is_error":true,"result":"HTTP 403 Forbidden: authentication failed"}"#,
        ]
        .join("\n"),
    )
    .expect("write full.md");

    let mut meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    meta.exit_code = 0;
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let artifact: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(session_dir.join("output").join("review-verdict.json"))
            .expect("read verdict"),
    )
    .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Unavailable);
    assert_eq!(artifact.verdict_legacy, "UNAVAILABLE");
    assert_ne!(artifact.decision, ReviewDecision::Pass);
    assert_ne!(artifact.verdict_legacy, "CLEAN");
    assert!(
        artifact
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("HTTP 403"))
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1930_plain_prose_quoted_terminal_error_payload_persists_pass() {
    let session_id = "01TEST1930QUOTEDJSONPASS";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1930-quoted-json-pass", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("full.md"),
        [
            "<!-- CSA:SECTION:summary -->",
            "PASS",
            "<!-- CSA:SECTION:summary:END -->",
            "<!-- CSA:SECTION:details -->",
            "No blocking issues found. The reviewed code quotes this terminal-error payload:",
            "```json",
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"result","subtype":"error_api","is_error":true,"result":"HTTP 403 Forbidden: authentication failed"}"#,
            "```",
            "<!-- CSA:SECTION:details:END -->",
        ]
        .join("\n"),
    )
    .expect("write full.md");

    let mut meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    meta.exit_code = 0;
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let artifact: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(session_dir.join("output").join("review-verdict.json"))
            .expect("read verdict"),
    )
    .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert_ne!(artifact.decision, ReviewDecision::Unavailable);
    assert_ne!(artifact.verdict_legacy, "UNAVAILABLE");
    assert!(artifact.failure_reason.is_none());

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1716_successful_zero_findings_fallback_with_prior_failure_still_passes() {
    let session_id = "01TEST1716SUCCESSFALLBACK00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1716-successful-fallback", session_id);

    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write real findings.toml");

    let mut meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    meta.exit_code = 0;
    meta.primary_failure = Some("QUOTA_EXHAUSTED".to_string());
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert_eq!(artifact.primary_failure.as_deref(), Some("QUOTA_EXHAUSTED"));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
