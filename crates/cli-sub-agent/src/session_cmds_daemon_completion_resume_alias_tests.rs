use super::*;
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use csa_session::{
    SessionResult, TaskContext, get_session_dir, load_result, save_result, save_session,
};
use tempfile::tempdir;

#[test]
fn finalize_daemon_completion_follows_late_resume_target_alias() -> Result<()> {
    let tmp = tempdir()?;
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = tmp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home)?;
    let _home_guard = ScopedEnvVarRestore::set("HOME", tmp.path());
    let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
    let project = tmp.path();

    let mut target =
        csa_session::create_session_fresh(project, Some("real target"), None, Some("codex"))?;
    let wrapper = csa_session::create_session_fresh(project, Some("daemon wrapper"), None, None)?;
    target.task_context = TaskContext {
        task_type: Some(REVIEW_FIX_FINDING_TASK_TYPE.to_string()),
        tier_name: None,
    };
    save_session(&target)?;
    let target_id = target.meta_session_id;
    let wrapper_id = wrapper.meta_session_id;
    assert_ne!(target_id, wrapper_id);
    let wrapper_dir = get_session_dir(project, &wrapper_id)?;
    csa_session::write_resume_target(project, &wrapper_id, &target_id)?;

    let now = chrono::Utc::now();
    let target_result = SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: "target result".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };
    save_result(project, &target_id, &target_result)?;
    fs::write(
        daemon_completion_path(&wrapper_dir),
        "exit_code = 1\nstatus = \"failure\"\n",
    )?;

    let finalized = finalize_daemon_completion_if_present(&wrapper_dir)?
        .expect("target result should be visible through wrapper alias");

    assert_eq!(finalized.status, "success");
    assert_eq!(finalized.exit_code, 0);
    assert!(
        !wrapper_dir
            .join(csa_session::result::RESULT_FILE_NAME)
            .exists(),
        "wrapper completion must not synthesize wrapper result.toml after alias appears"
    );
    let target_result = load_result(project, &target_id)?.expect("target result remains");
    assert_eq!(target_result.status, "success");
    assert_eq!(target_result.exit_code, 0);
    Ok(())
}
