use super::*;
use crate::test_env_lock::TEST_ENV_LOCK;
use csa_session::FindingsFile;
use csa_session::state::ReviewSessionMeta;
use std::path::PathBuf;
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
        review_mode: None,
        fix_convergence: None,
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

fn read_findings_toml(session_dir: &Path) -> FindingsFile {
    let findings_path = session_dir.join("output").join("findings.toml");
    toml::from_str(&fs::read_to_string(findings_path).expect("read findings.toml"))
        .expect("parse findings.toml")
}

fn read_verdict(session_dir: &Path) -> ReviewVerdictArtifact {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    serde_json::from_str(&fs::read_to_string(verdict_path).expect("read verdict"))
        .expect("parse verdict")
}

fn write_extracted_empty_findings(session_dir: &Path) {
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
    fs::write(
        session_dir
            .join("output")
            .join(crate::review_cmd::findings_toml::FINDINGS_TOML_EXTRACTED_MARKER),
        b"",
    )
    .expect("write extracted marker");
}

#[test]
fn issue_2017_fail_verdict_backfills_parsed_details_finding_over_empty_artifact() {
    let session_id = "01TEST2017BACKFILLFIND";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2017-backfill-finding", session_id);

    write_extracted_empty_findings(&session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
Review result: FAIL. One medium severity finding blocks the change.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. [Medium] `output/findings.toml` can be empty while the verdict fails (`crates/cli-sub-agent/src/review_cmd_output_consistency.rs:51`, confidence=0.91)
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].severity, Severity::Medium);
    assert_eq!(
        findings.findings[0].file_ranges[0].path,
        "crates/cli-sub-agent/src/review_cmd_output_consistency.rs"
    );
    assert_eq!(findings.findings[0].file_ranges[0].start, 51);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2017_fail_verdict_with_unparseable_details_writes_explicit_artifact_error() {
    let session_id = "01TEST2017ARTIFACTERR";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2017-artifact-error", session_id);

    write_extracted_empty_findings(&session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
Review result: FAIL. A finding exists but the structured artifact is empty.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. Medium correctness regression remains but this line intentionally lacks a parseable delimiter.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(
        verdict.failure_reason.as_deref(),
        Some("prose_findings_present_but_unparsed")
    );
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].id, "artifact-generation-001");
    assert_eq!(findings.findings[0].severity, Severity::Medium);
    assert!(
        findings.findings[0]
            .description
            .contains("Artifact generation failed")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2017_extracted_empty_summary_only_fail_writes_artifact_error() {
    let session_id = "01TEST2017SUMMARYFAIL";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2017-summary-fail", session_id);

    write_extracted_empty_findings(&session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
Review verdict: FAIL. One blocking correctness issue remains.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
The reviewer reported a blocking failure but did not emit a parseable findings list.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(
        verdict.failure_reason.as_deref(),
        Some("fail_verdict_empty_findings_artifact")
    );

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].id, "artifact-generation-001");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2017_pass_without_findings_keeps_empty_findings_artifact_allowed() {
    let session_id = "01TEST2017PASSEMPTY000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2017-pass-empty", session_id);

    write_extracted_empty_findings(&session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
Review result: PASS. No findings.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

No blocking findings found.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert!(verdict.severity_counts.values().all(|count| *count == 0));

    let findings = read_findings_toml(&session_dir);
    assert!(findings.findings.is_empty());

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
