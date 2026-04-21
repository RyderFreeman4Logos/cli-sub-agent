use super::persist_review_verdict;
use crate::test_env_lock::TEST_ENV_LOCK;
use csa_core::types::ReviewDecision;
use csa_session::state::ReviewSessionMeta;
use csa_session::{
    Finding, FindingsFile, ReviewFinding, ReviewFindingFileRange, ReviewVerdictArtifact, Severity,
    write_findings_toml,
};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::OwnedMutexGuard;

fn make_review_meta(
    session_id: &str,
    decision: ReviewDecision,
    verdict: &str,
) -> ReviewSessionMeta {
    ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: String::new(),
        decision: decision.as_str().to_string(),
        verdict: verdict.to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "codex".to_string(),
        scope: "diff".to_string(),
        exit_code: if decision == ReviewDecision::Pass {
            0
        } else {
            1
        },
        fix_attempted: true,
        fix_rounds: 1,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
    }
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

fn make_review_finding(severity: Severity, id: &str) -> ReviewFinding {
    ReviewFinding {
        id: id.to_string(),
        severity,
        file_ranges: vec![ReviewFindingFileRange {
            path: "src/lib.rs".to_string(),
            start: 1,
            end: Some(1),
        }],
        is_regression_of_commit: None,
        suggested_test_scenario: None,
        description: format!("description {id}"),
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
fn persist_verdict_refreshes_on_fix_reuse_session() {
    let session_id = "01TESTREFRESHFIXREUSE000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-refresh-fix-reuse", session_id);
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let stale_artifact = ReviewVerdictArtifact::from_parts(
        session_id,
        ReviewDecision::Fail,
        "HAS_ISSUES",
        &[make_finding(Severity::High, "stale")],
        Vec::new(),
    );
    fs::write(
        &verdict_path,
        serde_json::to_vec_pretty(&stale_artifact).expect("serialize stale verdict"),
    )
    .expect("write stale verdict");

    write_findings_toml(
        &session_dir,
        &FindingsFile {
            findings: Vec::new(),
        },
    )
    .expect("write current findings.toml");
    let full_output = [json!({"type":"item.completed","item":{
        "id":"item_1",
        "type":"agent_message",
        "text":"<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nNo blocking issues found in this scope.\nOverall risk: low\n<!-- CSA:SECTION:details:END -->"
    }})]
    .into_iter()
    .map(|line| serde_json::to_string(&line).expect("serialize transcript line"))
    .collect::<Vec<_>>()
    .join("\n");
    fs::write(session_dir.join("output").join("full.md"), full_output)
        .expect("write full output transcript");

    let meta = make_review_meta(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&0));
    assert_eq!(artifact.severity_counts.get(&Severity::Critical), Some(&0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_verdict_prefers_session_findings_toml_over_root_review_findings_json() {
    let session_id = "01TESTFINDINGSTOMLPREFERRED0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("persist-review-verdict-findings-toml-preferred", session_id);
    let stale_root = json!({
        "findings": [make_finding(Severity::High, "stale-root")],
        "severity_summary": csa_session::SeveritySummary::from_findings(&[make_finding(Severity::High, "stale-root")]),
        "schema_version": "1.0",
        "session_id": session_id,
        "timestamp": chrono::Utc::now(),
        "overall_risk": "high"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&stale_root).expect("serialize stale root findings"),
    )
    .expect("write stale root findings");
    write_findings_toml(
        &session_dir,
        &FindingsFile {
            findings: vec![make_review_finding(Severity::Medium, "current-medium")],
        },
    )
    .expect("write current findings.toml");

    let meta = make_review_meta(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.severity_counts.get(&Severity::Medium), Some(&1));
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&0));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
