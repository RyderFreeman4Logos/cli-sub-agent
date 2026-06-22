use super::*;
use crate::session_cmds_daemon::{
    WaitBehavior, WaitLoopTiming, WaitReconciliationOutcome, handle_session_wait_with_hooks,
};
use crate::test_env_lock::TEST_ENV_LOCK;
use tempfile::tempdir;

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe {
            match self.original.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

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

/// Regression test for #1439: when the wait cap fires and the session daemon is
/// still alive, the wait must exit with code 0 (KV-warm), not 124 (legacy
/// timeout). The accompanying `CSA:SESSION_WAIT_KV_WARM` marker is verified by
/// code inspection in `session_cmds_daemon_wait.rs`; this test pins the
/// exit-code contract that callers (especially AI agents in `run_in_background`
/// task-notification loops) depend on.
#[cfg(target_os = "linux")]
#[test]
fn handle_session_wait_kv_warm_exit_when_daemon_alive_at_cap() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session =
        create_session(project, Some("wait-kv-warm-alive"), None, Some("codex")).expect("create");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");

    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn child");
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(child.id()),
    )
    .expect("write daemon pid");
    assert!(csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir));

    let exit_code = handle_session_wait_with_hooks(
        session_id,
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 0,
            memory_warn_mb: None,
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(1),
                memory_sample_interval: std::time::Duration::from_secs(15),
            },
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |_sid, _status, _exit_code, _synthetic, _mirror_to_stdout| {
            panic!("alive-at-cap path must not emit a completion signal");
        },
    )
    .expect("wait should reach KV-warm exit");

    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(
        exit_code, 0,
        "live daemon at wait cap must emit KV-warm exit (0), not legacy timeout (124)"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn handle_session_wait_kv_warm_after_registry_state_loss_with_metadata_fallback() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let caller_project = td.path().join("caller");
    let owner_project = td.path().join("owner");
    std::fs::create_dir_all(&caller_project).expect("create caller project");
    std::fs::create_dir_all(&owner_project).expect("create owner project");

    let session = create_session(
        &owner_project,
        Some("wait-metadata-only"),
        None,
        Some("codex"),
    )
    .expect("create");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(&owner_project, &session_id).expect("session dir");
    std::fs::remove_file(session_dir.join("state.toml")).expect("remove registry state");

    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn child");
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(child.id()),
    )
    .expect("write daemon pid");
    assert!(csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir));

    let exit_code = handle_session_wait_with_hooks(
        session_id,
        Some(caller_project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 0,
            memory_warn_mb: None,
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(1),
                memory_sample_interval: std::time::Duration::from_secs(15),
            },
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("registry-loss live session must not be reconciled")
        },
        |_sid, _status, _exit_code, _synthetic, _mirror_to_stdout| {
            panic!("alive-at-cap path must not emit a completion signal");
        },
    )
    .expect("wait should reach KV-warm exit through metadata fallback");

    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(
        exit_code, 0,
        "metadata-only exact fallback must keep CSA:SESSION_STARTED ids waitable after KV warm"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn stale_precheck_does_not_fail_live_daemon_session() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-stale-precheck-live-daemon"),
        None,
        Some("codex"),
    )
    .expect("create");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    let mut state = load_session(project, &session_id).expect("load session");
    state.phase = SessionPhase::Active;
    state.last_accessed = chrono::Utc::now() - chrono::Duration::seconds(7_200);
    save_session(&state).expect("save stale active session");

    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn child");
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(child.id()),
    )
    .expect("write daemon pid");
    assert!(csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir));

    let exit_code = handle_session_wait_with_hooks(
        session_id,
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 0,
            memory_warn_mb: None,
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(1),
                memory_sample_interval: std::time::Duration::from_secs(15),
            },
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |_sid, _status, _exit_code, _synthetic, _mirror_to_stdout| {
            panic!("stale precheck must not emit a synthetic completion for a live daemon");
        },
    )
    .expect("wait should skip stale precheck for live daemon");

    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(
        exit_code, 0,
        "live daemon must not be pre-failed solely because last_accessed is stale"
    );
}

#[test]
fn wait_retries_blank_state_during_startup_precheck() {
    assert_wait_retries_initializing_state_until_complete("\n  \n");
}

#[test]
fn wait_retries_partial_state_missing_required_field_during_startup_precheck() {
    assert_wait_retries_initializing_state_until_complete(
        "project_path = \"/tmp/wait-partial-state\"\n",
    );
}

#[cfg(unix)]
#[test]
fn wait_errors_on_malformed_state_past_startup_precheck_window() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-malformed-state-old"),
        None,
        Some("codex"),
    )
    .expect("create");
    let session_id = session.meta_session_id;
    let state_path = get_session_dir(project, &session_id)
        .expect("session dir")
        .join("state.toml");
    std::fs::write(&state_path, "not valid toml = [\n").expect("write malformed state");
    super::set_file_mtime_seconds_ago(&state_path, 2);

    let exit_code = handle_session_wait_with_hooks(
        session_id,
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(1),
                memory_sample_interval: std::time::Duration::from_secs(15),
            },
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("malformed state must fail before reconciliation")
        },
        |_sid, _status, _exit_code, _synthetic, _mirror_to_stdout| {
            panic!("malformed state must not emit completion")
        },
    )
    .expect("old malformed state should be reported as a wait failure");

    assert_eq!(
        exit_code, 1,
        "old malformed state must remain a hard wait failure"
    );
}

fn assert_wait_retries_initializing_state_until_complete(initial_state: &str) {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-initializing-state"),
        None,
        Some("codex"),
    )
    .expect("create");
    let session_id = session.meta_session_id;
    save_result(project, &session_id, &make_result("success", 0)).expect("save result");

    let state_path = get_session_dir(project, &session_id)
        .expect("session dir")
        .join("state.toml");
    let complete_state = std::fs::read_to_string(&state_path).expect("read complete state");
    std::fs::write(&state_path, initial_state).expect("write initializing state");

    let restore_path = state_path.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(restore_path, complete_state).expect("restore complete state");
    });

    let exit_code = handle_session_wait_with_hooks(
        session_id,
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(1),
                memory_sample_interval: std::time::Duration::from_secs(15),
            },
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("completed result should be loaded without reconciliation")
        },
        |_sid, _status, _exit_code, _synthetic, _mirror_to_stdout| {},
    )
    .expect("wait should retry initializing state and then read completed result");

    writer.join().expect("state restore thread");
    assert_eq!(exit_code, 0);
}
