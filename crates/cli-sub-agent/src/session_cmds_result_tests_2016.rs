use super::*;

#[cfg(target_os = "linux")]
#[test]
fn handle_session_result_uses_legacy_complete_marker_143_even_while_daemon_alive()
-> anyhow::Result<()> {
    use std::process::Command;

    let tmp = tempdir()?;
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = tmp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home)?;
    let _home_guard = EnvVarGuard::set("HOME", tmp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = tmp.path();

    let session = create_session(
        project,
        Some("result-legacy-complete-marker-live"),
        None,
        Some("codex"),
    )?;
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id)?;
    std::fs::write(session_dir.join(".complete"), "143\n")?;

    let mut child = Command::new("sleep").arg("5").spawn()?;
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(child.id())?,
    )?;
    assert!(csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir));

    handle_session_result(
        session_id.clone(),
        false,
        Some(project.to_string_lossy().into_owned()),
        StructuredOutputOpts::default(),
    )?;

    let result = load_result(project, &session_id)?
        .expect("session result should synthesize from legacy .complete");
    assert_eq!(result.status, "signal");
    assert_eq!(result.exit_code, 143);

    let persisted = csa_session::load_session(project, &session_id)?;
    assert_eq!(persisted.phase, csa_session::SessionPhase::Retired);
    assert_eq!(
        persisted.termination_reason.as_deref(),
        Some("legacy_complete_marker")
    );

    child.kill().ok();
    let _ = child.wait();
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn handle_session_result_retires_existing_result_after_legacy_complete_marker_with_live_daemon_pid()
-> anyhow::Result<()> {
    use std::process::Command;

    let tmp = tempdir()?;
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = tmp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home)?;
    let _home_guard = EnvVarGuard::set("HOME", tmp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = tmp.path();

    let session = create_session(
        project,
        Some("result-existing-legacy-complete-marker-live"),
        None,
        Some("codex"),
    )?;
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id)?;
    csa_session::save_result(
        project,
        &session_id,
        &SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "summary".to_string(),
            tool: "codex".to_string(),
            ..Default::default()
        },
    )?;
    std::fs::write(session_dir.join(".complete"), "143\n")?;

    let mut child = Command::new("sleep").arg("5").spawn()?;
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(child.id())?,
    )?;
    assert!(csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir));

    handle_session_result(
        session_id.clone(),
        false,
        Some(project.to_string_lossy().into_owned()),
        StructuredOutputOpts::default(),
    )?;

    let result =
        load_result(project, &session_id)?.expect("existing result should remain authoritative");
    assert_eq!(result.status, "success");
    assert_eq!(result.exit_code, 0);

    let persisted = csa_session::load_session(project, &session_id)?;
    assert_eq!(persisted.phase, csa_session::SessionPhase::Retired);
    assert_eq!(persisted.termination_reason.as_deref(), Some("completed"));

    child.kill().ok();
    let _ = child.wait();
    Ok(())
}
