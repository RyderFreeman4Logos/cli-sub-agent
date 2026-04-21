use super::*;
use crate::session_cmds_daemon::{
    SESSION_WAIT_MEMORY_WARN_EXIT_CODE, WaitBehavior, WaitLoopTiming, WaitReconciliationOutcome,
    handle_session_wait_with_hooks, handle_session_wait_with_hooks_and_sampler,
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

#[cfg(unix)]
#[test]
fn handle_session_wait_retires_active_session_after_dead_failure_completion_packet() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-retire-dead-completion-packet"),
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

    let exit_code = handle_session_wait(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        1,
    )
    .unwrap();

    assert_eq!(exit_code, 1);

    let result = load_result(project, &session_id)
        .unwrap()
        .expect("wait should synthesize a terminal result for a dead active session");
    assert_eq!(result.status, "failure");

    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(persisted.phase, SessionPhase::Retired);
    assert_eq!(
        persisted.termination_reason.as_deref(),
        Some("orphaned_process")
    );
}

#[test]
fn handle_session_wait_prefers_synthetic_failure_status_and_exit_code_over_completion_packet() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-synthetic-exit-code"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write completion packet");

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: true,
                synthetic: true,
            })
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should succeed");

    assert_eq!(exit_code, 1);
    assert_eq!(
        emitted_completion,
        Some((session_id, "failure".to_string(), 1, true))
    );
}

#[test]
fn handle_session_wait_prefers_late_real_result_status_and_exit_code_over_completion_packet() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(project, Some("wait-late-real-result"), None, Some("codex"))
        .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write completion packet");

    let late_result = SessionResult {
        summary: "late real terminal result".to_string(),
        ..make_result("failure", 7)
    };

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |project_root, current_session_id, _trigger| {
            save_result(project_root, current_session_id, &late_result).expect("save late result");
            Ok(WaitReconciliationOutcome {
                result_became_available: true,
                synthetic: false,
            })
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should succeed");

    assert_eq!(exit_code, 7);
    assert_eq!(
        emitted_completion,
        Some((session_id.clone(), "failure".to_string(), 7, false))
    );

    let persisted = load_result(project, &session_id)
        .expect("load result")
        .expect("late real result should persist");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 7);
}

#[test]
fn handle_session_wait_prefers_refreshed_real_result_status_and_exit_code_over_completion_packet() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-refresh-real-result"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write completion packet");
    #[cfg(unix)]
    set_file_mtime_seconds_ago(&session_dir.join("daemon-completion.toml"), 2);

    let refreshed_result = SessionResult {
        summary: "refreshed real terminal result".to_string(),
        ..make_result("failure", 7)
    };
    save_result(project, &session_id, &refreshed_result).expect("save refreshed result");

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("refresh_result_for_wait should short-circuit before reconcile");
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should succeed");

    assert_eq!(exit_code, 7);
    assert_eq!(
        emitted_completion,
        Some((session_id.clone(), "failure".to_string(), 7, false))
    );

    let persisted = load_result(project, &session_id)
        .expect("load result")
        .expect("refreshed real result should persist");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 7);
}

#[cfg(unix)]
#[test]
fn handle_session_wait_prefers_refreshed_real_result_on_equal_mtime_with_completion_packet() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-refresh-real-result-equal-mtime"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    let completion_path = session_dir.join("daemon-completion.toml");
    std::fs::write(&completion_path, "exit_code = 0\nstatus = \"success\"\n")
        .expect("write completion packet");

    let refreshed_result = SessionResult {
        summary: "refreshed real terminal result with equal mtime".to_string(),
        ..make_result("failure", 7)
    };
    save_result(project, &session_id, &refreshed_result).expect("save refreshed result");

    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    let same_second_time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    for path in [completion_path.as_path(), result_path.as_path()] {
        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open file for mtime update");
        file.set_times(std::fs::FileTimes::new().set_modified(same_second_time))
            .expect("set mtime");
    }

    let completion_modified = std::fs::metadata(&completion_path)
        .expect("completion metadata")
        .modified()
        .expect("completion modified time");
    let result_modified = std::fs::metadata(&result_path)
        .expect("result metadata")
        .modified()
        .expect("result modified time");
    assert_eq!(
        result_modified, completion_modified,
        "test setup requires equal mtimes"
    );

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("refresh_result_for_wait should short-circuit before reconcile");
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should succeed");

    assert_eq!(exit_code, 7);
    assert_eq!(
        emitted_completion,
        Some((session_id.clone(), "failure".to_string(), 7, false))
    );

    let persisted = load_result(project, &session_id)
        .expect("load result")
        .expect("refreshed real result should persist");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 7);
}

#[cfg(unix)]
#[test]
fn handle_session_wait_errors_when_refresh_branch_cannot_persist_retired_phase() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-refresh-retire-save-failure"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write completion packet");
    set_file_mtime_seconds_ago(&session_dir.join("daemon-completion.toml"), 2);

    let refreshed_result = SessionResult {
        summary: "refreshed real terminal result".to_string(),
        ..make_result("failure", 7)
    };
    save_result(project, &session_id, &refreshed_result).expect("save refreshed result");

    let lock_path = session_dir.join(".reconcile.lock");
    std::fs::create_dir(&lock_path).expect("poison reconcile lock path with a directory");

    let mut emitted_completion = false;
    let wait_err = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("refresh_result_for_wait should short-circuit before reconcile");
        },
        |_sid: &str, _status: &str, _exit_code, _synthetic, _mirror_to_stdout| {
            emitted_completion = true;
        },
    )
    .expect_err("wait should fail when retire persistence fails");

    assert!(
        wait_err
            .to_string()
            .contains("Failed to open reconciliation lock file"),
        "unexpected error: {wait_err:#}"
    );
    assert!(
        !emitted_completion,
        "wait should not emit completion when retire persistence fails"
    );

    let persisted = load_session(project, &session_id).expect("load session");
    assert_eq!(persisted.phase, SessionPhase::Active);
    assert_eq!(persisted.termination_reason, None);

    let result = load_result(project, &session_id)
        .expect("load result")
        .expect("refreshed real result should still exist");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 7);
}

#[cfg(target_os = "linux")]
#[test]
fn test_session_wait_memory_warn_emits_marker_and_exits_33() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(project, Some("wait-memory-warn"), None, Some("codex"))
        .expect("create session");
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

    let mut emitted_marker: Option<String> = None;
    let exit_code = handle_session_wait_with_hooks_and_sampler(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 5,
            memory_warn_mb: Some(64),
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(1),
                memory_sample_interval: std::time::Duration::ZERO,
            },
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |_sid: &str, _status: &str, _exit_code, _synthetic, _mirror_to_stdout| {
            panic!("memory warn must not emit completion");
        },
        |_project_root, _session_id| Ok(65),
        |sid, rss_mb, limit_mb| {
            emitted_marker = Some(format!(
                "<!-- CSA:MEMORY_WARN session={} rss_mb={} limit_mb={} -->",
                sid, rss_mb, limit_mb
            ));
        },
    )
    .expect("memory warn should return early");

    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(exit_code, SESSION_WAIT_MEMORY_WARN_EXIT_CODE);
    assert_eq!(
        emitted_marker,
        Some(format!(
            "<!-- CSA:MEMORY_WARN session={} rss_mb=65 limit_mb=64 -->",
            session_id
        ))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_session_wait_memory_warn_disabled_when_zero() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(project, Some("wait-memory-disabled"), None, Some("codex"))
        .expect("create session");
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

    let mut sample_calls = 0;
    let exit_code = handle_session_wait_with_hooks_and_sampler(
        session_id,
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 0,
            memory_warn_mb: Some(0),
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(1),
                memory_sample_interval: std::time::Duration::ZERO,
            },
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |_sid: &str, _status: &str, _exit_code, _synthetic, _mirror_to_stdout| {
            panic!("disabled sampler should time out before completion");
        },
        |_project_root, _session_id| {
            sample_calls += 1;
            Ok(1)
        },
        |_sid, _rss_mb, _limit_mb| {
            panic!("disabled sampler must not emit marker");
        },
    )
    .expect("wait should fall back to timeout");

    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(exit_code, 124);
    assert_eq!(sample_calls, 0);
}

#[cfg(target_os = "linux")]
#[test]
fn test_session_wait_procfs_failure_fallback() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-memory-procfs-fail"),
        None,
        Some("codex"),
    )
    .expect("create session");
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

    let mut sample_calls = 0;
    let exit_code = handle_session_wait_with_hooks_and_sampler(
        session_id,
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 0,
            memory_warn_mb: Some(1),
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(1),
                memory_sample_interval: std::time::Duration::ZERO,
            },
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |_sid: &str, _status: &str, _exit_code, _synthetic, _mirror_to_stdout| {
            panic!("sampling failure fallback should not emit completion");
        },
        |_project_root, _session_id| {
            sample_calls += 1;
            Err(std::io::Error::other("procfs unavailable"))
        },
        |_sid, _rss_mb, _limit_mb| {
            panic!("sampling failure fallback must not emit marker");
        },
    )
    .expect("wait should fall back to classic timeout");

    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(exit_code, 124);
    assert_eq!(sample_calls, 1);
}
