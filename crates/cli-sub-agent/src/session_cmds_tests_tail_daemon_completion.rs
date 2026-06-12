use super::super::*;

#[cfg(unix)]
#[test]
fn handle_session_wait_fails_closed_on_success_completion_without_result() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-success-completion-without-result"),
        None,
        Some("codex"),
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    backdate_tree(&session_dir, 120);
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .unwrap();

    let exit_code = handle_session_wait(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        1,
    )
    .unwrap();

    assert_eq!(exit_code, 1);
    let result = load_result(project, &session_id)
        .unwrap()
        .expect("wait should synthesize fail-closed result");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.raw_process_exit_code, Some(0));
}
