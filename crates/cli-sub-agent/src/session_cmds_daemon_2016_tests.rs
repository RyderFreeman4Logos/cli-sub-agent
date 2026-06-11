use super::*;
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
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
fn attach_consumes_legacy_complete_marker_even_with_live_daemon_pid() {
    use std::sync::mpsc;
    use std::time::Duration;

    let td = tempfile::tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = ScopedEnvVarRestore::set("HOME", td.path());
    let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = csa_session::create_session(
        project,
        Some("attach-legacy-complete-marker"),
        None,
        Some("opencode"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = csa_session::get_session_dir(project, &session_id).expect("session dir");
    std::fs::write(session_dir.join("stdout.log"), "").expect("write stdout log");
    std::fs::write(session_dir.join(".complete"), "143\n").expect("write legacy marker");

    let mut child = Command::new("sleep").arg("5").spawn().expect("spawn child");
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(child.id()),
    )
    .expect("write daemon pid");
    assert!(csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir));

    let (tx, rx) = mpsc::channel();
    let attach_session = session_id.clone();
    let attach_project = project.to_string_lossy().into_owned();
    let handle = std::thread::spawn(move || {
        let result = handle_session_attach(
            attach_session,
            false,
            Some(attach_project),
            &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV,
        )
        .map_err(|err| err.to_string());
        let _ = tx.send(result);
    });

    let exit_code = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("attach should consume legacy .complete without waiting on daemon.pid")
        .expect("attach result");
    child.kill().ok();
    let _ = child.wait();
    handle.join().expect("attach thread join");
    assert_eq!(exit_code, 143);
}

#[test]
fn attach_reactivation_cleanup_removes_legacy_complete_marker() {
    let td = tempfile::tempdir().expect("tempdir");
    let session_dir = td.path();
    std::fs::write(session_dir.join(".complete"), "143\n").expect("write legacy marker");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 1\nstatus = \"failure\"\n",
    )
    .expect("write completion packet");

    clear_attach_reactivation_artifacts(session_dir).expect("cleanup");

    assert!(!session_dir.join(".complete").exists());
    assert!(!session_dir.join("daemon-completion.toml").exists());
}
