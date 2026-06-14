use super::*;

#[cfg(unix)]
#[test]
fn session_to_json_reports_no_live_pid_for_active_session_when_progress_blocks_reconcile() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let _daemon_project_guard = EnvVarGuard::set("CSA_DAEMON_PROJECT_ROOT", "");
    let _daemon_dir_guard = EnvVarGuard::set("CSA_DAEMON_SESSION_DIR", "");
    let project = td.path();

    let created = create_session(project, Some("json-no-live-pid"), None, None).unwrap();
    let session_id = created.meta_session_id.clone();
    let session_dir = get_session_dir(project, &session_id).unwrap();
    std::fs::write(session_dir.join("output.log"), "fresh output bytes\n").unwrap();
    assert!(
        !csa_process::ToolLiveness::has_live_process(&session_dir),
        "fixture must not have a live tool lock PID"
    );
    assert!(
        !csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir),
        "fixture must not have a live daemon PID"
    );

    let session = load_session(project, &session_id).unwrap();
    let value = session_to_json(&session);

    assert_eq!(
        value.get("status").and_then(|v| v.as_str()),
        Some("NoLivePID")
    );
    assert!(
        load_result(project, &session_id).unwrap().is_none(),
        "fresh output should still block synthetic result creation"
    );
}
