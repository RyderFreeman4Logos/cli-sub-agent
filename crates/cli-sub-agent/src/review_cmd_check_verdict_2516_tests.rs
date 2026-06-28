use super::*;
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use chrono::Utc;
use csa_core::types::ReviewDecision;
use csa_core::vcs::{VcsIdentity, VcsKind};
use csa_session::state::ReviewSessionMeta;
use csa_session::{
    FindingsFile, ReviewFinding, ReviewFindingFileRange, ReviewVerdictArtifact, Severity,
};
use tempfile::TempDir;

#[test]
fn issue_2516_check_verdict_rejects_pass_json_with_findings_toml() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let branch = "feature";
    let head_sha = "abcdef1234567890";
    let mut session = csa_session::create_session_fresh(&project, Some("review"), None, None)
        .expect("create session");
    session.branch = Some(branch.to_string());
    session.git_head_at_creation = Some(head_sha.to_string());
    session.vcs_identity = Some(VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some(head_sha.to_string()),
        change_id: None,
        short_id: Some(short_sha(head_sha).to_string()),
        ref_name: Some(branch.to_string()),
        op_id: None,
    });
    csa_session::save_session(&session).expect("save session");
    let session_dir = csa_session::get_session_dir(&project, &session.meta_session_id).unwrap();
    let meta = ReviewSessionMeta {
        session_id: session.meta_session_id.clone(),
        head_sha: head_sha.to_string(),
        decision: ReviewDecision::Pass.as_str().to_string(),
        verdict: "CLEAN".to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "codex".to_string(),
        scope: REQUIRED_FULL_DIFF_SCOPE.to_string(),
        exit_code: 0,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: Utc::now(),
        diff_fingerprint: None,
        review_mode: None,
        fix_convergence: None,
    };
    csa_session::state::write_review_meta(&session_dir, &meta).expect("write review meta");
    csa_session::write_review_verdict(
        &session_dir,
        &ReviewVerdictArtifact::from_parts(
            session.meta_session_id.clone(),
            ReviewDecision::Pass,
            "CLEAN",
            &[],
            Vec::new(),
        ),
    )
    .expect("write verdict");
    csa_session::write_findings_toml(
        &session_dir,
        &FindingsFile {
            findings: vec![ReviewFinding {
                id: "F1".to_string(),
                severity: Severity::High,
                file_ranges: vec![ReviewFindingFileRange {
                    path: "src/lib.rs".to_string(),
                    start: 7,
                    end: None,
                }],
                is_regression_of_commit: None,
                suggested_test_scenario: None,
                description: "contradictory blocking finding".to_string(),
            }],
        },
    )
    .expect("write findings.toml");

    let found = check_review_verdict_for_target(
        &project,
        branch,
        head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .expect("check verdict should run");

    assert!(found.is_none());
}
