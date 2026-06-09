use chrono::TimeDelta;
use csa_core::types::{ReviewDecision, ToolName};
use csa_session::ReviewVerdictArtifact;
use csa_session::state::ReviewSessionMeta;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::OwnedMutexGuard;

use super::persist_review_verdict;
use crate::test_env_lock::TEST_ENV_LOCK;

fn make_review_meta_with_decision(
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
        tool: ToolName::Codex.as_str().to_string(),
        scope: "diff".to_string(),
        exit_code: match decision {
            ReviewDecision::Pass | ReviewDecision::Skip => 0,
            ReviewDecision::Fail | ReviewDecision::Uncertain | ReviewDecision::Unavailable => 1,
        },
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        review_mode: None,
        fix_convergence: None,
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

fn read_verdict(session_dir: &Path) -> ReviewVerdictArtifact {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
        .expect("parse verdict")
}

fn write_empty_findings_toml(session_dir: &Path) {
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
}

fn persist_summary(session_dir: &Path, summary: &str) {
    csa_session::persist_structured_output(
        session_dir,
        &format!("<!-- CSA:SECTION:summary -->\n{summary}\n<!-- CSA:SECTION:summary:END -->\n"),
    )
    .expect("persist summary");
}

fn success_result(summary: &str) -> csa_session::SessionResult {
    let now = chrono::Utc::now();
    csa_session::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: summary.to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now + TimeDelta::seconds(65),
        ..Default::default()
    }
}

fn persist_pass_meta_review(
    test_name: &str,
    session_id: &str,
    summary: &str,
) -> (OwnedMutexGuard<()>, PathBuf, PathBuf) {
    let (env_lock, project_root, session_dir) = lock_test_session(test_name, session_id);
    write_empty_findings_toml(&session_dir);
    persist_summary(&session_dir, summary);

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    (env_lock, project_root, session_dir)
}

fn assert_summary_fails_canonical_result(test_name: &str, session_id: &str, summary: &str) {
    let (_env_lock, project_root, session_dir) =
        persist_pass_meta_review(test_name, session_id, summary);

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");

    let mut result = success_result(summary);
    let changed = crate::session_observability::enrich_result_from_session_dir(
        &project_root,
        session_id,
        &session_dir,
        &mut result,
    )
    .expect("enrich session result");

    assert!(changed, "review verdict must repair the success result");
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.status, "failure");

    let wait_summary =
        crate::session_cmds_daemon::render_wait_result_summary(&session_dir, session_id, &result);
    assert!(wait_summary.contains("Status: failure"));
    assert!(wait_summary.contains("Exit code: 1"));
    assert!(wait_summary.contains("Review verdict: FAIL"));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1981_high_severity_summary_repairs_success_result_to_failure() {
    assert_summary_fails_canonical_result(
        "issue-1981-high-severity-summary",
        "01KTMNVNV7092C3QHBHT21N09E",
        "Reviewed `main...HEAD` in read-only mode. Found 1 high-severity issue: `--memory-max-mb` can be accepted and used for admission projection while silently not applying a sandbox memory limit on default-off lightweight/tool-config paths.",
    );
}

#[test]
fn issue_1982_medium_correctness_fail_summary_repairs_success_result_to_failure() {
    assert_summary_fails_canonical_result(
        "issue-1982-medium-correctness-summary",
        "01KTMR3Y88834S8ENRC5WYCVWW",
        "One medium correctness finding remains after re-verifying the prior stale-FTS assumption. The rejudge-specific hard-delete path is fixed, but the same physical-delete bug still exists in the general database delete/purge paths.  FAIL",
    );
}

#[test]
fn clean_pass_summary_preserves_success_result() {
    let session_id = "01KTMZZZZZZZZZZZZZZZZZZZZZ";
    let summary = "PASS: no high or medium severity issues remain after review.";
    let (_env_lock, project_root, session_dir) =
        persist_pass_meta_review("issue-1981-clean-pass-control", session_id, summary);

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");

    let mut result = success_result(summary);
    crate::session_observability::enrich_result_from_session_dir(
        &project_root,
        session_id,
        &session_dir,
        &mut result,
    )
    .expect("enrich session result");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.status, "success");

    let wait_summary =
        crate::session_cmds_daemon::render_wait_result_summary(&session_dir, session_id, &result);
    assert!(wait_summary.contains("Status: success"));
    assert!(wait_summary.contains("Exit code: 0"));
    assert!(wait_summary.contains("Review verdict: PASS"));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
