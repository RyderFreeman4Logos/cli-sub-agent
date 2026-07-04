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

#[test]
fn issue_2601_pass_summary_chinese_positive_evidence_keeps_review_verdict_pass() {
    let session_id = "01TEST2601CHINESEPASS";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2601-chinese-positive-evidence", session_id);

    write_extracted_empty_findings(&session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        concat!(
            "<!-- CSA:SECTION:summary -->\n",
            "PASS\n",
            "<!-- CSA:SECTION:summary:END -->\n\n",
            "<!-- CSA:SECTION:details -->\n",
            "\u{7ed3}\u{8bba}\u{ff1a}\u{672a}\u{53d1}\u{73b0}",
            "\u{9700}\u{8981}\u{963b}\u{65ad}\u{5408}\u{5e76}\u{7684}",
            "\u{6b63}\u{786e}\u{6027}\u{3001}\u{5b89}\u{5168}\u{6216}",
            "\u{5951}\u{7ea6}\u{95ee}\u{9898}\u{3002}\n\n",
            "- P1/P2/C1: \u{9ed8}\u{8ba4} evidence \u{5173}\u{95ed}\u{3001}raw ",
            "\u{5173}\u{95ed}\u{3001}XDG \u{9ed8}\u{8ba4}\u{8def}\u{5f84}",
            "\u{4e0e}\u{8def}\u{5f84}\u{8986}\u{76d6}/\u{975e}\u{6cd5}",
            "\u{8def}\u{5f84}\u{6821}\u{9a8c}\u{5728} settings \u{4e2d}",
            "\u{5df2}\u{5b9e}\u{73b0}\u{5e76}\u{6d4b}\u{8bd5}\n",
            "- P2: CLI \u{900f}\u{4f20}\u{5df2}\u{6709}\u{76f4}\u{63a5}",
            "\u{6d4b}\u{8bd5}\n",
            "- C1: \u{975e}\u{6cd5}\u{8def}\u{5f84}\u{5df2}\u{901a}\u{8fc7} ",
            "settings \u{6821}\u{9a8c}\u{7f13}\u{89e3}\n",
            "- P1: fallback \u{884c}\u{4e3a}\u{5df2}\u{6709}\u{673a}\u{68b0}",
            "\u{6d4b}\u{8bd5}\n",
            "- P2: reviewer summary \u{5df2}\u{8986}\u{76d6}\n",
            "<!-- CSA:SECTION:details:END -->\n",
        ),
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert!(verdict.severity_counts.values().all(|count| *count == 0));

    let findings = read_findings_toml(&session_dir);
    assert!(findings.findings.is_empty());

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
