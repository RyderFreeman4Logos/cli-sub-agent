use super::{
    ToolReviewFailureKind, derive_decision_from_text, detect_tool_review_failure,
    ensure_review_summary_artifact, extract_review_text, persist_review_verdict,
};
use crate::review_cmd::output::artifacts::PersistedReviewArtifact;
use crate::test_env_lock::TEST_ENV_LOCK;
use csa_core::types::{ReviewDecision, ToolName};
use csa_session::state::ReviewSessionMeta;
use csa_session::{Finding, ReviewArtifact, ReviewVerdictArtifact, Severity, SeveritySummary};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::OwnedMutexGuard;

fn make_review_meta(session_id: &str) -> ReviewSessionMeta {
    ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: String::new(),
        decision: ReviewDecision::Fail.as_str().to_string(),
        verdict: "HAS_ISSUES".to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "codex".to_string(),
        scope: "diff".to_string(),
        exit_code: 1,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
    }
}

fn make_review_meta_with_decision(
    session_id: &str,
    decision: ReviewDecision,
    verdict: &str,
) -> ReviewSessionMeta {
    let mut meta = make_review_meta(session_id);
    meta.decision = decision.as_str().to_string();
    meta.verdict = verdict.to_string();
    meta
}

fn make_finding(severity: Severity, fid: &str) -> Finding {
    Finding {
        severity,
        fid: fid.to_string(),
        file: "src/lib.rs".to_string(),
        line: Some(1),
        rule_id: format!("rule.{fid}"),
        summary: format!("summary {fid}"),
        engine: "reviewer".to_string(),
    }
}

fn temp_project_root(test_name: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("csa-{test_name}-{suffix}"));
    fs::create_dir_all(&path).expect("create temp project root");
    path
}

fn create_session_dir(project_root: &Path, session_id: &str) -> PathBuf {
    let session_dir = csa_session::get_session_root(project_root)
        .expect("resolve session root")
        .join("sessions")
        .join(session_id);
    fs::create_dir_all(session_dir.join("output")).expect("create session output dir");
    session_dir
}

fn lock_test_session(test_name: &str, session_id: &str) -> (OwnedMutexGuard<()>, PathBuf, PathBuf) {
    let env_lock = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let project_root = temp_project_root(test_name);
    let session_dir = create_session_dir(&project_root, session_id);
    (env_lock, project_root, session_dir)
}

#[test]
fn derive_decision_from_text_skip_token_beats_clean_phrase() {
    let decision = derive_decision_from_text(
        "summary=skip\nNo blocking issues found in this scope.",
        &BTreeMap::new(),
        Some("low"),
    );

    assert_eq!(decision, ReviewDecision::Skip);
}

#[test]
fn derive_decision_from_text_uncertain_without_findings_stays_uncertain() {
    let decision = derive_decision_from_text(
        "summary=uncertain\nReview did not complete.",
        &BTreeMap::new(),
        Some("low"),
    );

    assert_eq!(decision, ReviewDecision::Uncertain);
}

#[test]
fn derive_decision_from_text_clean_phrase_without_skip_stays_pass() {
    let decision = derive_decision_from_text(
        "No blocking issues found in this scope.\nOverall risk: low",
        &BTreeMap::new(),
        Some("low"),
    );

    assert_eq!(decision, ReviewDecision::Pass);
}

#[test]
fn extract_review_text_skips_leading_non_json_preamble() {
    let transcript = concat!(
        "warning: provider banner\n",
        "stdout noise before transcript\n",
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"<!-- CSA:SECTION:summary -->\\nFAIL\\n<!-- CSA:SECTION:summary:END -->\"}}\n"
    );

    assert_eq!(
        extract_review_text(transcript).as_deref(),
        Some("<!-- CSA:SECTION:summary -->\nFAIL\n<!-- CSA:SECTION:summary:END -->")
    );
}

#[test]
fn derive_decision_from_text_high_risk_without_findings_fails() {
    let decision = derive_decision_from_text(
        "PASS\nNo blocking issues found in this scope.\nOverall risk: high",
        &BTreeMap::new(),
        Some("high"),
    );

    assert_eq!(decision, ReviewDecision::Fail);
}

#[test]
fn detect_tool_review_failure_flags_gemini_oauth_prompt_without_real_turn() {
    let stdout = "Opening authentication page\nDo you want to continue? [Y/n]\n";
    let detected = detect_tool_review_failure(ToolName::GeminiCli, stdout, "");
    assert_eq!(
        detected,
        Some(ToolReviewFailureKind::GeminiAuthPromptDetected)
    );
}

#[test]
fn detect_tool_review_failure_ignores_normal_review_output() {
    let stdout = concat!(
        "{\"type\":\"turn.completed\",\"turn_id\":\"turn_123\"}\n",
        "<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n",
        "output_tokens: 12\n"
    );
    assert!(detect_tool_review_failure(ToolName::GeminiCli, stdout, "").is_none());
}

#[test]
fn detect_tool_review_failure_never_fires_for_non_gemini_tools() {
    let stdout = "Opening authentication page\nDo you want to continue? [Y/n]\n";
    assert!(detect_tool_review_failure(ToolName::Codex, stdout, "").is_none());
}

#[test]
fn detect_tool_review_failure_handles_guarded_browser_prompt_variant() {
    let stdout = concat!(
        "<csa-caller-sa-guard>\n",
        "SA MODE ACTIVE\n",
        "</csa-caller-sa-guard>\n",
        "\n",
        "Opening authentication page in your browser. Do you want to continue? [Y/n]: ",
        "<csa-caller-sa-guard>\n",
        "SA MODE ACTIVE\n",
        "</csa-caller-sa-guard>\n",
    );
    let stderr =
        "[stdout] Opening authentication page in your browser. Do you want to continue? [Y/n]: \n";
    let detected = detect_tool_review_failure(ToolName::GeminiCli, stdout, stderr);
    assert_eq!(
        detected,
        Some(ToolReviewFailureKind::GeminiAuthPromptDetected)
    );
}

#[test]
fn persist_review_verdict_prefers_structured_findings_summary() {
    let session_id = "01TESTFINDINGS000000000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-findings", session_id);
    let findings_path = session_dir.join("review-findings.json");
    let findings = vec![
        make_finding(Severity::High, "high"),
        make_finding(Severity::Low, "low"),
    ];
    let artifact = ReviewArtifact {
        severity_summary: SeveritySummary::from_findings(&findings),
        findings: findings.clone(),
        review_mode: None,
        schema_version: "1.0".to_string(),
        session_id: session_id.to_string(),
        timestamp: chrono::Utc::now(),
    };
    let artifact = json!({
        "findings": artifact.findings,
        "severity_summary": artifact.severity_summary,
        "review_mode": artifact.review_mode,
        "schema_version": artifact.schema_version,
        "session_id": artifact.session_id,
        "timestamp": artifact.timestamp,
        "overall_risk": "high"
    });
    fs::write(
        &findings_path,
        serde_json::to_vec_pretty(&artifact).expect("serialize findings"),
    )
    .expect("write findings artifact");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&1));
    assert_eq!(artifact.severity_counts.get(&Severity::Low), Some(&1));
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_reconciles_high_risk_no_findings_with_text_fallback() {
    let session_id = "01TESTRISKRECONCILE00000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-high-risk-no-findings", session_id);
    let artifact = json!({
        "findings": [],
        "severity_summary": SeveritySummary::default(),
        "schema_version": "1.0",
        "session_id": session_id,
        "timestamp": chrono::Utc::now(),
        "overall_risk": "high"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("serialize findings"),
    )
    .expect("write findings artifact");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_recounts_findings_when_summary_is_zeroed() {
    let session_id = "01TESTRECOUNT00000000000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-recount-findings", session_id);
    let findings = vec![
        make_finding(Severity::Critical, "critical"),
        make_finding(Severity::Medium, "medium"),
        make_finding(Severity::Medium, "medium-2"),
    ];
    let artifact = json!({
        "findings": findings,
        "severity_summary": SeveritySummary::default(),
        "schema_version": "1.0",
        "session_id": session_id,
        "timestamp": chrono::Utc::now(),
        "overall_risk": "high"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("serialize findings"),
    )
    .expect("write findings artifact");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.severity_counts.get(&Severity::Critical), Some(&1));
    assert_eq!(artifact.severity_counts.get(&Severity::Medium), Some(&2));
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&0));
    assert_eq!(artifact.severity_counts.get(&Severity::Low), Some(&0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_falls_back_to_full_output_transcript_counts() {
    let session_id = "01TESTFULL0000000000000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-full-output", session_id);
    let full_output = [
        json!({"type":"thread.started","thread_id":"thread-1"}),
        json!({"type":"item.completed","item":{
            "id":"item_1",
            "type":"agent_message",
            "text":"<!-- CSA:SECTION:summary -->\nFAIL\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nFindings\n1. [High][regression] first\n2. [Medium][test-gap] second\n3. [High][correctness] third\n4. [Info][maintainability] fourth\n\nOverall risk: high\n<!-- CSA:SECTION:details:END -->"
        }}),
    ]
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
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&2));
    assert_eq!(artifact.severity_counts.get(&Severity::Medium), Some(&1));
    assert_eq!(artifact.severity_counts.get(&Severity::Low), Some(&1));
    assert_eq!(artifact.severity_counts.get(&Severity::Critical), Some(&0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_falls_back_to_priority_markers_in_full_output() {
    let session_id = "01TESTPRIORITYMARKERS000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-priority-markers", session_id);
    let full_output = [json!({"type":"item.completed","item":{
        "id":"item_1",
        "type":"agent_message",
        "text":"<!-- CSA:SECTION:summary -->\nFAIL\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nFindings\n1. [P0][correctness] first\n2. [P1][regression] second\n3. [P2][test-gap] third\n4. [P3][style] fourth\n5. [P4][nit] fifth\n6. [Info][maintainability] sixth\n\nOverall risk: high\n<!-- CSA:SECTION:details:END -->"
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
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.severity_counts.get(&Severity::Critical), Some(&1));
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&1));
    assert_eq!(artifact.severity_counts.get(&Severity::Medium), Some(&1));
    assert_eq!(artifact.severity_counts.get(&Severity::Low), Some(&3));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn ensure_review_summary_artifact_synthesizes_summary_from_details_only_output() {
    let session_id = "01TESTSUMMARYSYNTH000000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("review-summary-synthesis", session_id);
    let details_only =
        "<!-- CSA:SECTION:details -->\nFAIL\nDetailed body\n<!-- CSA:SECTION:details:END -->\n";
    csa_session::persist_structured_output(&session_dir, details_only).expect("persist details");

    ensure_review_summary_artifact(&session_dir, details_only).expect("synthesize summary");

    let summary = csa_session::read_section(&session_dir, "summary")
        .expect("read summary section")
        .expect("summary should exist");
    assert_eq!(summary, "FAIL");

    let index = csa_session::load_output_index(&session_dir)
        .expect("load output index")
        .expect("index should exist");
    assert_eq!(
        index.sections.first().map(|section| section.id.as_str()),
        Some("summary")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

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
fn persist_review_verdict_empty_structured_findings_preserve_uncertain_meta() {
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
    assert_eq!(artifact.decision, ReviewDecision::Uncertain);
    assert_eq!(artifact.verdict_legacy, "UNCERTAIN");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[path = "review_cmd_output_prose_clean_tests.rs"]
mod prose_clean_tests;

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

    let meta = make_review_meta(session_id);
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

    let meta = make_review_meta(session_id);
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
