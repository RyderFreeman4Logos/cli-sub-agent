use super::*;
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use csa_core::types::ReviewDecision;
use csa_core::vcs::{VcsIdentity, VcsKind};
use tempfile::TempDir;

#[test]
fn daemon_completion_before_result_preserves_exact_head_review_availability() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let mut session = csa_session::create_session_fresh(
        &project,
        Some("review: range:main...HEAD"),
        None,
        Some("codex"),
    )
    .expect("create daemon review session");
    session.branch = Some("feature".to_string());
    session.git_head_at_creation = Some("abcdef1234567890".to_string());
    session.vcs_identity = Some(VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some("abcdef1234567890".to_string()),
        change_id: None,
        short_id: Some("abcdef123456".to_string()),
        ref_name: Some("feature".to_string()),
        op_id: None,
    });
    session.task_context = csa_session::TaskContext {
        task_type: Some("review".to_string()),
        tier_name: None,
    };
    csa_session::save_session(&session).expect("save daemon review session state");
    let session_id = session.meta_session_id.clone();
    let session_dir = csa_session::get_session_dir(&project, &session_id).unwrap();
    csa_session::write_review_verdict(
        &session_dir,
        &ReviewVerdictArtifact::from_parts(
            session_id.clone(),
            ReviewDecision::Pass,
            "CLEAN",
            &[],
            Vec::new(),
        ),
    )
    .expect("write review verdict before result.toml");

    let _daemon_id = ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_ID", &session_id);
    let _daemon_dir = ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_DIR", &session_dir);
    let _daemon_project = ScopedEnvVarRestore::set("CSA_DAEMON_PROJECT_ROOT", &project);
    crate::session_cmds_daemon::persist_daemon_completion_from_env(1);

    let found = check_review_verdict_for_target(
        &project,
        "feature",
        "abcdef1234567890",
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .unwrap()
    .expect("existing review verdict should remain available for exact-head gates");
    assert_eq!(found.session_id, session_id);

    let result = csa_session::load_result(&project, &session_id)
        .unwrap()
        .expect("daemon completion should publish a result from review artifacts");
    assert_eq!(result.status, "success");
    assert_eq!(result.exit_code, 0);
}
