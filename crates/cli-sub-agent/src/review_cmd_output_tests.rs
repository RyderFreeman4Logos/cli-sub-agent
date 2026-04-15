use super::{PersistedReviewArtifact, persist_review_verdict};
use csa_core::types::ReviewDecision;
use csa_session::state::ReviewSessionMeta;
use csa_session::{Finding, ReviewArtifact, ReviewVerdictArtifact, Severity, SeveritySummary};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn make_review_meta(session_id: &str) -> ReviewSessionMeta {
    ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: String::new(),
        decision: ReviewDecision::Fail.as_str().to_string(),
        verdict: "HAS_ISSUES".to_string(),
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
    let session_dir =
        csa_session::get_session_dir(project_root, session_id).expect("resolve session dir");
    fs::create_dir_all(session_dir.join("output")).expect("create session output dir");
    session_dir
}

#[test]
fn persist_review_verdict_skips_when_ai_file_exists() {
    let project_root = temp_project_root("persist-review-verdict-skip");
    let session_id = "01TESTSKIP0000000000000000";
    let session_dir = create_session_dir(&project_root, session_id);
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let ai_payload = r#"{"ai":"preserved"}"#;
    fs::write(&verdict_path, ai_payload).expect("write AI verdict artifact");

    let meta = make_review_meta(session_id);
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let actual = fs::read_to_string(&verdict_path).expect("read verdict artifact");
    assert_eq!(actual, ai_payload);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_prefers_structured_findings_summary() {
    let project_root = temp_project_root("persist-review-verdict-findings");
    let session_id = "01TESTFINDINGS000000000000";
    let session_dir = create_session_dir(&project_root, session_id);
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
fn persist_review_verdict_falls_back_to_full_output_transcript_counts() {
    let project_root = temp_project_root("persist-review-verdict-full-output");
    let session_id = "01TESTFULL0000000000000000";
    let session_dir = create_session_dir(&project_root, session_id);
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
    assert_eq!(artifact.severity_counts.get(&Severity::Info), Some(&1));
    assert_eq!(artifact.severity_counts.get(&Severity::Low), Some(&0));
    assert_eq!(artifact.severity_counts.get(&Severity::Critical), Some(&0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_falls_back_to_priority_markers_in_full_output() {
    let project_root = temp_project_root("persist-review-verdict-priority-markers");
    let session_id = "01TESTPRIORITYMARKERS000000";
    let session_dir = create_session_dir(&project_root, session_id);
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
    assert_eq!(artifact.severity_counts.get(&Severity::Low), Some(&2));
    assert_eq!(artifact.severity_counts.get(&Severity::Info), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_marks_clean_transcript_as_pass() {
    let project_root = temp_project_root("persist-review-verdict-pass");
    let session_id = "01TESTPASS0000000000000000";
    let session_dir = create_session_dir(&project_root, session_id);
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
fn persist_review_verdict_plain_text_full_output_falls_back_to_review_meta_findings() {
    let project_root = temp_project_root("persist-review-verdict-meta-fallback");
    let session_id = "01TESTMETAFALLBACK000000000";
    let session_dir = create_session_dir(&project_root, session_id);
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
    assert_eq!(artifact.severity_counts.get(&Severity::Info), Some(&0));
    assert_eq!(artifact.severity_counts.get(&Severity::Critical), Some(&0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_concrete_findings_override_uncertain_token() {
    let project_root = temp_project_root("persist-review-verdict-concrete-over-uncertain");
    let session_id = "01TESTCONCRETEOVERUNCERTAIN";
    let session_dir = create_session_dir(&project_root, session_id);
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
    let project_root = temp_project_root("persist-review-verdict-empty-findings-uncertain");
    let session_id = "01TESTEMPTYFINDINGSUNCERTAIN";
    let session_dir = create_session_dir(&project_root, session_id);
    let findings_path = session_dir.join("review-findings.json");
    let artifact = json!({
        "findings": [],
        "severity_summary": { "critical": 0, "high": 0, "medium": 0, "low": 0, "info": 0 },
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

#[test]
fn persist_review_verdict_json_transcript_without_review_message_falls_back_to_review_meta() {
    let project_root = temp_project_root("persist-review-verdict-json-no-review-message");
    let session_id = "01TESTJSONNOREVIEWMESSAGE00";
    let session_dir = create_session_dir(&project_root, session_id);
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
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert!(artifact.severity_counts.values().all(|value| *value == 0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persisted_review_artifact_deserializes_optional_overall_risk() {
    let artifact: PersistedReviewArtifact = serde_json::from_value(json!({
        "findings": [],
        "severity_summary": { "critical": 0, "high": 0, "medium": 0, "low": 0, "info": 0 },
        "overall_risk": "low"
    }))
    .expect("deserialize persisted review artifact");
    assert_eq!(artifact.overall_risk.as_deref(), Some("low"));
}
