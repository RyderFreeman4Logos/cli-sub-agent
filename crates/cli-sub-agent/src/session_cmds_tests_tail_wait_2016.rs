use super::*;
use crate::test_env_lock::ScopedEnvVarRestore;
use std::process::Command;

#[cfg(target_os = "linux")]
fn read_process_start_time_ticks(pid: u32) -> u64 {
    let stat_path = format!("/proc/{pid}/stat");
    let content = std::fs::read_to_string(stat_path).expect("read /proc stat");
    let close_paren = content.rfind(')').expect("stat comm terminator");
    let after_comm = &content[close_paren + 1..];
    let mut parts = after_comm.split_whitespace();
    parts.next().expect("state");
    parts.next().expect("ppid");
    parts.next().expect("pgrp");
    for _ in 0..16 {
        parts.next().expect("intermediate stat field");
    }
    parts
        .next()
        .expect("starttime")
        .parse::<u64>()
        .expect("starttime parse")
}

#[cfg(target_os = "linux")]
fn daemon_pid_record(pid: u32) -> String {
    format!("{pid} {}\n", read_process_start_time_ticks(pid))
}

#[cfg(target_os = "linux")]
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
    stale_session.last_accessed = chrono::Utc::now() - chrono::Duration::seconds(7_200);
    save_session(&stale_session).unwrap();
    std::fs::write(session_dir.join(".complete"), "143\n").unwrap();
    let mut child = Command::new("sleep").arg("5").spawn().unwrap();
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(child.id()),
    )
    .unwrap();
    assert!(csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir));

    let exit_code = handle_session_wait(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        1,
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

    child.kill().ok();
    let _ = child.wait();
}

#[cfg(target_os = "linux")]
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
    let mut child = Command::new("sleep").arg("5").spawn().unwrap();
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(child.id()),
    )
    .unwrap();
    assert!(csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir));

    let exit_code = handle_session_wait(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        1,
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

    child.kill().ok();
    let _ = child.wait();
}
