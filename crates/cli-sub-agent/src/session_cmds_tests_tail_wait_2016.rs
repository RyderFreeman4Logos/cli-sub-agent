use super::*;
use crate::session_cmds_daemon::{WaitBehavior, WaitLoopTiming, handle_session_wait_with_hooks_at};
use crate::test_env_lock::ScopedEnvVarRestore;

#[test]
fn handle_session_wait_retires_active_session_after_legacy_complete_marker_143_even_with_live_daemon_pid()
 {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = ScopedEnvVarRestore::set("HOME", td.path());
    let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-retire-legacy-complete-marker"),
        None,
        Some("codex"),
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let mut stale_session = load_session(project, &session_id).unwrap();
    stale_session.last_accessed = chrono::DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
        .expect("fixed stale event")
        .with_timezone(&chrono::Utc);
    save_session(&stale_session).unwrap();
    std::fs::write(session_dir.join(".complete"), "143\n").unwrap();

    let fixed_now = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
        .expect("fixed liveness clock")
        .with_timezone(&chrono::Utc);
    let exit_code = handle_session_wait_with_hooks_at(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        fixed_now,
        Some(true),
        |_project_root, _current_session_id, _trigger| {
            panic!("legacy complete marker must resolve before dead-session reconciliation")
        },
        |_sid, _status, _exit_code, _synthetic, _mirror_to_stdout| {},
    )
    .unwrap();

    assert_eq!(exit_code, 1);

    let result = load_result(project, &session_id)
        .unwrap()
        .expect("wait should synthesize a terminal result from .complete");
    assert_eq!(result.status, "signal");
    assert_eq!(result.exit_code, 143);

    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(persisted.phase, SessionPhase::Retired);
    assert_eq!(
        persisted.termination_reason.as_deref(),
        Some("legacy_complete_marker")
    );
}

#[test]
fn handle_session_wait_retires_existing_result_after_legacy_complete_marker_with_live_daemon_pid() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = ScopedEnvVarRestore::set("HOME", td.path());
    let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-retire-existing-result-legacy-complete"),
        None,
        Some("codex"),
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    save_result(project, &session_id, &make_result("success", 0)).unwrap();
    std::fs::write(session_dir.join(".complete"), "143\n").unwrap();

    let fixed_now = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
        .expect("fixed liveness clock")
        .with_timezone(&chrono::Utc);
    let exit_code = handle_session_wait_with_hooks_at(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        fixed_now,
        Some(true),
        |_project_root, _current_session_id, _trigger| {
            panic!("legacy complete marker must resolve before dead-session reconciliation")
        },
        |_sid, _status, _exit_code, _synthetic, _mirror_to_stdout| {},
    )
    .unwrap();

    assert_eq!(exit_code, 0);
    let result = load_result(project, &session_id)
        .unwrap()
        .expect("existing result should remain authoritative");
    assert_eq!(result.status, "success");
    assert_eq!(result.exit_code, 0);

    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(persisted.phase, SessionPhase::Retired);
    assert_eq!(persisted.termination_reason.as_deref(), Some("completed"));
}
