use super::*;

#[test]
fn detached_debate_initialization_prepares_state_session_directories_before_spawn() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let project_root = temp.path().join("project");
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&project_root).expect("project root");
    let _home_guard = crate::test_env_lock::ScopedEnvVarRestore::set("HOME", temp.path());
    let _state_guard =
        crate::test_env_lock::ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);

    prepare_detached_debate_initialization(&project_root)
        .expect("detached debate initialization should prepare session directories");

    let session_root =
        csa_session::get_session_root(&project_root).expect("session root should resolve");
    assert!(
        session_root.join("sessions").is_dir(),
        "detached debate preflight should create the sessions directory"
    );
}

#[test]
fn detached_debate_initialization_names_xdg_state_file_conflict() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let project_root = temp.path().join("project");
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&project_root).expect("project root");
    std::fs::write(&state_home, "conflicting file").expect("state conflict fixture");
    let _home_guard = crate::test_env_lock::ScopedEnvVarRestore::set("HOME", temp.path());
    let _state_guard =
        crate::test_env_lock::ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);

    let error = prepare_detached_debate_initialization(&project_root)
        .expect_err("state file conflict must fail before daemon detaches");
    let message = format!("{error:#}");

    assert!(
        message.contains("detached debate initialization"),
        "{message}"
    );
    assert!(
        message.contains(&state_home.display().to_string()),
        "{message}"
    );
    assert!(message.contains("not a directory"), "{message}");
}
