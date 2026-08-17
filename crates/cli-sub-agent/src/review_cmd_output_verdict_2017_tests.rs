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
fn issue_2882_markdown_heading_findings_with_locations_backfill_structured_artifacts() {
    let session_id = "01TEST2882HEADINGFIND";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2882-markdown-heading-findings", session_id);

    write_extracted_empty_findings(&session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
Review result: FAIL. Two blocking findings remain.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

### 1. [HIGH][correctness] A committed mutation can be reported as a retryable lock failure.
Location: `src/core/sqlite_retry.rs:65`
Trigger: a retry wakes after the configured deadline.

### 2. [MEDIUM][regression] Self-healing connection open is outside the deadline.
Location: `src/core/async_db.rs:620`
Impact: a future retry change can reintroduce the deadline overrun.

## Recommended Actions

1. Enforce the deadline before scheduling another retry.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 2, "findings: {findings:#?}");
    assert_eq!(findings.findings[0].severity, Severity::High);
    assert_eq!(
        findings.findings[0].file_ranges[0].path,
        "src/core/sqlite_retry.rs"
    );
    assert_eq!(findings.findings[0].file_ranges[0].start, 65);
    assert_eq!(findings.findings[1].severity, Severity::Medium);
    assert_eq!(
        findings.findings[1].file_ranges[0].path,
        "src/core/async_db.rs"
    );
    assert_eq!(findings.findings[1].file_ranges[0].start, 620);

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));
    assert_ne!(
        verdict.failure_reason.as_deref(),
        Some("prose_findings_present_but_unparsed")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2664_priority_alias_prose_findings_reconcile_artifacts_and_result() {
    let session_id = "01M06C1K96HC53JD4149Z6TNGC";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2664-prose-findings", session_id);
    let rejected_fragment = "Parser diagnostic appendix remains preserved verbatim.";

    csa_session::persist_structured_output(
        &session_dir,
        &format!(
            r#"<!-- CSA:SECTION:summary -->
Decision: blocking findings present. Three high-severity defects and one medium regression remain.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. **[High/P1] Repeated pinned rebuild destroys the rollback artifact before the transaction starts**
   Location: `scripts/llm_guard_proxy_cached_rebuild.sh:102`

2. **[High/P1] The requested SHA still does not attest the bytes Cargo compiles**
   Location: `scripts/llm_guard_proxy_cached_rebuild.sh:88`

3. **[High/P1] Termination after publication bypasses rollback**
   Location: `scripts/llm_guard_proxy_cached_rebuild.sh:135`

4. **[Medium/P2] The lock path breaks the legacy custom SOURCE_DIR path on a fresh home**
   Location: `scripts/llm_guard_proxy_cached_rebuild.sh:49`

{rejected_fragment}
<!-- CSA:SECTION:details:END -->
"#
        ),
    )
    .expect("persist structured output");

    let mut meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    meta.status_reason = Some("prose_findings_present_but_unparsed".to_string());
    meta.failure_reason = Some("prose_findings_present_but_unparsed".to_string());
    csa_session::state::write_review_meta(&session_dir, &meta).expect("write stale review meta");
    csa_session::write_findings_toml(
        &session_dir,
        &FindingsFile {
            findings: vec![csa_session::ReviewFinding {
                id: "artifact-generation-001".to_string(),
                severity: Severity::Medium,
                file_ranges: Vec::new(),
                is_regression_of_commit: None,
                suggested_test_scenario: None,
                description: "Artifact generation failed: review verdict is FAIL but CSA could not extract a structured finding. Reason: prose_findings_present_but_unparsed. Inspect output/details.md and output/review-verdict.json.".to_string(),
            }],
        },
    )
    .expect("write stale placeholder findings");
    let mut stale_verdict = ReviewVerdictArtifact::from_parts(
        session_id.to_string(),
        ReviewDecision::Fail,
        "HAS_ISSUES".to_string(),
        &[],
        Vec::new(),
    );
    stale_verdict.severity_counts.insert(Severity::Medium, 2);
    stale_verdict.failure_reason = Some("prose_findings_present_but_unparsed".to_string());
    csa_session::write_review_verdict(&session_dir, &stale_verdict)
        .expect("write stale review verdict");

    let now = chrono::Utc::now();
    csa_session::save_result(
        &project_root,
        session_id,
        &csa_session::SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "Decision: blocking findings present.".to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            ..Default::default()
        },
    )
    .expect("write stale success result");

    let result = crate::session_observability::refresh_and_repair_result(&project_root, session_id)
        .expect("refresh session result")
        .expect("session result exists");
    let findings = read_findings_toml(&session_dir);
    let expected = [
        (Severity::High, 102),
        (Severity::High, 88),
        (Severity::High, 135),
        (Severity::Medium, 49),
    ];
    assert_eq!(findings.findings.len(), expected.len(), "{findings:#?}");
    for (finding, (severity, line)) in findings.findings.iter().zip(expected) {
        assert_eq!(finding.severity, severity);
        assert_eq!(
            finding.file_ranges[0].path,
            "scripts/llm_guard_proxy_cached_rebuild.sh"
        );
        assert_eq!(finding.file_ranges[0].start, line);
    }

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&3));
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));
    assert_eq!(verdict.failure_reason, None);

    let repaired_meta: ReviewSessionMeta = serde_json::from_str(
        &fs::read_to_string(session_dir.join("review_meta.json")).expect("read review meta"),
    )
    .expect("parse review meta");
    assert_eq!(repaired_meta.decision, ReviewDecision::Fail.as_str());
    assert_eq!(repaired_meta.verdict, "HAS_ISSUES");
    assert_eq!(repaired_meta.exit_code, 1);
    assert_eq!(repaired_meta.status_reason, None);
    assert_eq!(repaired_meta.failure_reason, None);
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);

    let wait_summary =
        crate::session_cmds_daemon::render_wait_result_summary(&session_dir, session_id, &result);
    assert!(
        wait_summary.contains("Review verdict: FAIL"),
        "{wait_summary}"
    );
    assert!(
        !wait_summary.contains("prose_findings_present_but_unparsed"),
        "{wait_summary}"
    );
    assert!(
        fs::read_to_string(session_dir.join("output").join("details.md"))
            .expect("read details")
            .contains(rejected_fragment)
    );

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
