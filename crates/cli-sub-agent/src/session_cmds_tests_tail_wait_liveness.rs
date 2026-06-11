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

/// When PID-level detection misses (session_has_terminal_process=false) but broader
/// filesystem liveness signals remain (is_alive=true due to recent session writes),
/// the wait must continue polling and eventually return the KV-warm exit (0) — not
/// failure (1). Original regression for #1396; updated for #1439 which reclassifies
/// the alive-at-cap exit code from 124 to 0.
#[test]
fn handle_session_wait_continues_polling_when_pid_missing_but_liveness_signals_present() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    // Create session with no daemon.pid or lock files — session_has_terminal_process returns
    // false. The session directory itself was just written (state.toml), so
    // ToolLiveness::is_alive returns true via has_recent_session_write_signal.
    let session = create_session(
        project,
        Some("wait-pid-missing-liveness-present"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");

    // Verify preconditions: no PID signals, but recent filesystem writes make is_alive=true.
    assert!(
        !csa_process::ToolLiveness::has_live_process(&session_dir),
        "test requires session_has_terminal_process=false: no live process"
    );
    assert!(
        !csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir),
        "test requires session_has_terminal_process=false: no daemon pid"
    );
    assert!(
        csa_process::ToolLiveness::is_alive(&session_dir),
        "recent session writes must make is_alive return true"
    );

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
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |_sid, _status, _exit_code, _synthetic, _mirror_to_stdout| {},
    )
    .expect("wait should succeed");

    assert_eq!(
        exit_code, 0,
        "wait must emit the KV-warm exit (0), not report failure (1), when liveness signals exist"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn handle_session_wait_defers_terminal_result_while_daemon_pid_still_alive() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-terminal-result-live-daemon-pid"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");

    let terminal_result = SessionResult {
        summary: "already completed successfully".to_string(),
        ..make_result("success", 0)
    };
    save_result(project, &session_id, &terminal_result).expect("save terminal result");

    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("spawn live daemon stand-in");
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(child.id()),
    )
    .expect("write live daemon pid");
    assert!(
        csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir),
        "test setup requires a live daemon pid signal"
    );

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let wait_result = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 0,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    );

    child.kill().ok();
    child.wait().ok();

    let exit_code = wait_result.expect("wait should succeed");
    assert_eq!(exit_code, 0);
    assert_eq!(
        emitted_completion, None,
        "live daemon sessions must not emit terminal completion from an intermediate result.toml"
    );
}
