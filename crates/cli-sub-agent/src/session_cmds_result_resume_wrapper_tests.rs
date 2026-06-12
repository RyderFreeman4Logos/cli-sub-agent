use super::*;

fn make_result(status: &str, exit_code: i32) -> SessionResult {
    let now = chrono::Utc::now();
    SessionResult {
        status: status.to_string(),
        exit_code,
        summary: status.to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        ..Default::default()
    }
}

fn move_session_to_legacy_root(project: &std::path::Path, session_id: &str) -> std::path::PathBuf {
    let primary_root = csa_session::get_session_root(project).unwrap();
    let primary_session_dir = primary_root.join("sessions").join(session_id);
    let primary_state_dir = csa_config::paths::state_dir_write().unwrap();
    let legacy_state_dir = csa_config::paths::legacy_state_dir().unwrap();
    let relative_root = primary_root.strip_prefix(&primary_state_dir).unwrap();
    let legacy_root = legacy_state_dir.join(relative_root);
    let legacy_sessions_dir = legacy_root.join("sessions");
    std::fs::create_dir_all(&legacy_sessions_dir).unwrap();
    let legacy_session_dir = legacy_sessions_dir.join(session_id);
    std::fs::rename(&primary_session_dir, &legacy_session_dir).unwrap();
    legacy_session_dir
}

#[cfg(unix)]
#[test]
fn handle_session_result_on_resume_wrapper_uses_worker_result() {
    let tmp = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = tmp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", tmp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = tmp.path();

    let worker = create_session(project, Some("worker-result"), None, Some("codex")).unwrap();
    let wrapper = create_session(project, Some("wrapper-result"), None, None).unwrap();
    let worker_id = worker.meta_session_id;
    let wrapper_id = wrapper.meta_session_id;
    let wrapper_dir = get_session_dir(project, &wrapper_id).unwrap();
    csa_session::write_resume_target(project, &wrapper_id, &worker_id).unwrap();
    std::fs::write(
        wrapper_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .unwrap();
    csa_session::save_result(
        project,
        &worker_id,
        &SessionResult {
            summary: "worker completed".to_string(),
            ..make_result("success", 0)
        },
    )
    .unwrap();

    handle_session_result(
        wrapper_id.clone(),
        false,
        Some(project.to_string_lossy().into_owned()),
        StructuredOutputOpts::default(),
    )
    .unwrap();

    assert!(
        load_result(project, &wrapper_id).unwrap().is_none(),
        "session result on a wrapper must not synthesize wrapper result.toml"
    );
    let result = load_result(project, &worker_id)
        .unwrap()
        .expect("worker result should be authoritative");
    assert_eq!(result.summary, "worker completed");
}

#[cfg(unix)]
#[test]
fn handle_session_result_on_resume_wrapper_follows_worker_in_legacy_root() {
    let tmp = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = tmp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", tmp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = tmp.path();

    let worker =
        create_session(project, Some("worker-result-legacy"), None, Some("codex")).unwrap();
    let wrapper = create_session(project, Some("wrapper-result"), None, None).unwrap();
    let worker_id = worker.meta_session_id;
    let wrapper_id = wrapper.meta_session_id;
    let wrapper_dir = get_session_dir(project, &wrapper_id).unwrap();
    let worker_dir = move_session_to_legacy_root(project, &worker_id);
    assert!(worker_dir.join("state.toml").is_file());

    csa_session::write_resume_target(project, &wrapper_id, &worker_id)
        .expect("resume wrapper alias should accept a legacy-root target");
    std::fs::write(
        wrapper_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .unwrap();
    csa_session::save_result(
        project,
        &worker_id,
        &SessionResult {
            summary: "legacy worker completed".to_string(),
            ..make_result("success", 0)
        },
    )
    .unwrap();

    handle_session_result(
        wrapper_id.clone(),
        false,
        Some(project.to_string_lossy().into_owned()),
        StructuredOutputOpts::default(),
    )
    .unwrap();

    assert!(
        load_result(project, &wrapper_id).unwrap().is_none(),
        "session result on a cross-root wrapper must not synthesize wrapper result.toml"
    );
    let result = load_result(project, &worker_id)
        .unwrap()
        .expect("legacy-root worker result should be authoritative");
    assert_eq!(result.summary, "legacy worker completed");
}
