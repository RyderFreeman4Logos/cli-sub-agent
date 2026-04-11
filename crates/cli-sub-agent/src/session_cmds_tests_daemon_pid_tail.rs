use super::*;

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
fn handle_session_wait_ignores_completion_packet_while_raw_daemon_pid_is_alive() {
    use std::process::Command;

    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-raw-daemon-pid-completion-packet"),
        None,
        Some("codex"),
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 1\nstatus = \"failure\"\n",
    )
    .unwrap();

    let mut child = Command::new("sleep").arg("5").spawn().unwrap();
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(child.id()),
    )
    .unwrap();
    assert!(
        !csa_process::ToolLiveness::has_live_process(&session_dir),
        "fixture should rely on raw daemon.pid liveness, not context-matched process detection"
    );
    assert!(csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir));

    let exit_code =
        handle_session_wait(session_id, Some(project.to_string_lossy().into_owned()), 1).unwrap();

    assert_eq!(
        exit_code, 124,
        "wait should time out instead of trusting daemon-completion while daemon.pid is still alive"
    );

    child.kill().ok();
    let _ = child.wait();
}

#[cfg(target_os = "linux")]
#[test]
fn handle_session_kill_rejects_daemon_pid_start_time_mismatch() {
    use std::process::Command;

    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("kill-raw-daemon-pid-mismatch"),
        None,
        Some("codex"),
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    let mut child = Command::new("sleep").arg("60").spawn().unwrap();
    std::fs::write(
        session_dir.join("daemon.pid"),
        format!("{} 0\n", child.id()),
    )
    .unwrap();

    let err =
        handle_session_kill(session_id, Some(project.to_string_lossy().into_owned())).unwrap_err();

    assert!(
        err.to_string().contains("potentially reused PID"),
        "unexpected error: {err:#}"
    );
    assert!(
        child.try_wait().unwrap().is_none(),
        "mismatched daemon pid record must not kill an unrelated process"
    );

    child.kill().ok();
    let _ = child.wait();
}
