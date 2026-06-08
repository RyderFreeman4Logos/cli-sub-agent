use super::*;
use crate::session_cmds_daemon::{WaitBehavior, WaitLoopTiming, handle_session_wait_with_hooks};
use crate::test_env_lock::TEST_ENV_LOCK;
use tempfile::tempdir;

#[test]
fn handle_session_wait_syncs_failed_review_verdict_before_printing_result() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-failed-review-verdict"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write stale success completion packet");
    let stale_success = SessionResult {
        summary: "review found blocking issues".to_string(),
        ..make_result("success", 0)
    };
    save_result(project, &session_id, &stale_success).expect("save stale success result");
    let failed_verdict = csa_session::ReviewVerdictArtifact::from_parts(
        session_id.clone(),
        csa_core::types::ReviewDecision::Fail,
        "HAS_ISSUES",
        &[],
        Vec::new(),
    );
    csa_session::write_review_verdict(&session_dir, &failed_verdict)
        .expect("write failed review verdict");

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("review verdict refresh should short-circuit before reconcile");
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should sync failed review verdict");

    assert_eq!(exit_code, 1);
    assert_eq!(
        emitted_completion,
        Some((session_id.clone(), "failure".to_string(), 1, false))
    );
    let persisted = load_result(project, &session_id)
        .expect("load result")
        .expect("result should remain terminal");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 1);
}
