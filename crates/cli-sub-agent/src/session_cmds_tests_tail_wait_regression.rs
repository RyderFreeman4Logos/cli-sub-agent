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

#[cfg(target_os = "linux")]
#[test]
fn handle_session_wait_completes_terminal_result_with_stale_daemon_pid() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-terminal-result-stale-daemon-pid"),
        None,
        Some("codex"),
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let terminal_result = SessionResult {
        summary: "already completed successfully".to_string(),
        ..make_result("success", 0)
    };
    save_result(project, &session_id, &terminal_result).unwrap();

    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    std::fs::write(
        session_dir.join("daemon.pid"),
        format!("{} 0\n", child.id()),
    )
    .unwrap();
    assert!(
        !csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir),
        "start-time mismatch must make daemon.pid stale"
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
            panic!("terminal result should short-circuit stale liveness reconciliation");
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    );

    child.kill().ok();
    child.wait().ok();

    let exit_code = wait_result.unwrap();
    assert_eq!(exit_code, 0);
    assert_eq!(
        emitted_completion,
        Some((session_id, "success".to_string(), 0, false))
    );
}

#[test]
fn handle_session_wait_detects_turn_scoped_output_result_fallback() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-turn-scoped-result-fallback"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    let turn_result_path = csa_session::turn_contract_result_path(&session_dir, 1);
    std::fs::create_dir_all(turn_result_path.parent().expect("turn result parent"))
        .expect("create turn result dir");
    let turn_result = SessionResult {
        summary: "turn-scoped result fallback".to_string(),
        ..make_result("success", 0)
    };
    let turn_result_toml =
        toml::to_string_pretty(&turn_result).expect("serialize turn-scoped result");
    std::fs::write(&turn_result_path, turn_result_toml).expect("write turn-scoped result");
    assert!(
        !session_dir
            .join(csa_session::result::RESULT_FILE_NAME)
            .exists(),
        "test setup requires missing root result.toml"
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
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should detect turn-scoped result fallback");

    assert_eq!(exit_code, 0);
    assert_eq!(
        emitted_completion,
        Some((session_id, "success".to_string(), 0, false))
    );
}

#[test]
fn handle_session_wait_does_not_reuse_prior_turn_output_result_fallback() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let mut session = create_session(
        project,
        Some("wait-does-not-reuse-prior-turn-result"),
        None,
        Some("codex"),
    )
    .expect("create session");
    session.turn_count = 1;
    save_session(&session).expect("save completed turn count");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    let prior_turn_result_path = csa_session::turn_contract_result_path(&session_dir, 1);
    std::fs::create_dir_all(prior_turn_result_path.parent().expect("turn result parent"))
        .expect("create prior turn result dir");
    let prior_turn_result = SessionResult {
        summary: "prior turn result must not be reused".to_string(),
        ..make_result("success", 0)
    };
    let prior_turn_result_toml =
        toml::to_string_pretty(&prior_turn_result).expect("serialize prior turn result");
    std::fs::write(&prior_turn_result_path, prior_turn_result_toml)
        .expect("write prior turn result");
    assert!(
        !csa_session::turn_contract_result_path(&session_dir, 2).exists(),
        "test setup requires missing expected turn 2 result"
    );
    assert!(
        !session_dir
            .join(csa_session::result::RESULT_FILE_NAME)
            .exists(),
        "test setup requires missing root result.toml"
    );

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let _exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
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
    )
    .expect("wait should not treat prior turn output as current completion");

    assert_eq!(
        emitted_completion, None,
        "wait must not emit a success completion from turn 1 while waiting for turn 2"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn handle_session_wait_ignores_intermediate_success_result_while_daemon_pid_alive() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-intermediate-success-live-daemon"),
        None,
        Some("codex"),
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    save_result(
        project,
        &session_id,
        &SessionResult {
            summary: "intermediate self-report success".to_string(),
            ..make_result("success", 0)
        },
    )
    .unwrap();

    let mut child = std::process::Command::new("sleep")
        .arg("0.2")
        .spawn()
        .unwrap();
    std::fs::write(
        session_dir.join("daemon.pid"),
        format!("{} 0\n", child.id()),
    )
    .unwrap();
    assert!(
        !csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir),
        "start-time mismatch must make daemon.pid stale"
    );
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(child.id()),
    )
    .unwrap();
    assert!(csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir));

    let session_dir_for_writer = session_dir.clone();
    let failure_result = SessionResult {
        summary: "POST-EXEC GATE FAILED (exit=1, step=just find-monolith-files)".to_string(),
        ..make_result("failure", 1)
    };
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let encoded = toml::to_string_pretty(&failure_result).unwrap();
        std::fs::write(session_dir_for_writer.join("result.toml"), encoded).unwrap();
        std::fs::write(
            session_dir_for_writer.join("daemon-completion.toml"),
            "exit_code = 1\nstatus = \"failure\"\n",
        )
        .unwrap();
    });

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let wait_result = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(5),
                memory_sample_interval: std::time::Duration::from_secs(15),
            },
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

    writer.join().unwrap();
    child.wait().ok();

    let exit_code = wait_result.unwrap();
    assert_eq!(
        exit_code, 1,
        "wait must not return the intermediate success while daemon gates are still running"
    );
    assert_eq!(
        emitted_completion,
        Some((session_id, "failure".to_string(), 1, false))
    );
}

#[test]
fn handle_session_wait_does_not_complete_daemon_packet_without_terminal_result() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-completion-packet-no-result"),
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
    std::fs::write(
        session_dir.join("stderr.log"),
        "session still writing diagnostics\n",
    )
    .unwrap();
    assert!(
        csa_process::ToolLiveness::is_alive(&session_dir),
        "recent diagnostics should keep this Active session nonterminal"
    );

    let mut emitted_completion = false;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
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
        |_sid: &str, _status: &str, _exit_code, _synthetic, _mirror_to_stdout| {
            emitted_completion = true;
        },
    )
    .unwrap();

    assert_eq!(
        exit_code, 0,
        "Active no-result sessions should use the nonterminal KV-warm path"
    );
    assert!(
        !emitted_completion,
        "daemon completion packet alone must not emit SESSION_WAIT_COMPLETED"
    );
    assert!(
        load_result(project, &session_id).unwrap().is_none(),
        "test setup must remain without a terminal result"
    );
}

#[test]
fn handle_session_wait_ignores_tier_failover_superseded_result_while_liveness_present() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-tier-failover-superseded-live"),
        None,
        Some("gemini-cli"),
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    save_result(
        project,
        &session_id,
        &SessionResult {
            status: "tier_failover_superseded".to_string(),
            exit_code: 1,
            summary: "status: 400".to_string(),
            tool: "gemini-cli".to_string(),
            ..make_result("failure", 1)
        },
    )
    .unwrap();
    std::fs::write(
        session_dir.join("stderr.log"),
        "fallback codex review is being scheduled\n",
    )
    .unwrap();
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 1\nstatus = \"failure\"\n",
    )
    .unwrap();
    let pending_result = load_result(project, &session_id)
        .unwrap()
        .expect("superseded result should exist");
    assert!(
        crate::session_tier_failover::is_pending_tier_failover_handoff(
            &session_dir,
            &pending_result
        ),
        "same state must make session result report pending tier fallback"
    );

    let mut emitted_completion = false;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
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
        |_sid: &str, _status: &str, _exit_code, _synthetic, _mirror_to_stdout| {
            emitted_completion = true;
        },
    )
    .unwrap();

    assert_eq!(
        exit_code, 0,
        "pending tier fallback should use the nonterminal KV-warm path"
    );
    assert!(
        !emitted_completion,
        "superseded intermediate attempts must not emit terminal wait completion while fallback is live"
    );
}

#[test]
fn handle_session_wait_terminalizes_last_candidate_gemini_failure() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-tier-failover-gemini-400-live"),
        None,
        Some("gemini-cli"),
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let mut session_state = load_session(project, &session_id).unwrap();
    session_state.task_context = TaskContext {
        task_type: Some("reviewer_sub_session".to_string()),
        tier_name: Some("tier-4-critical".to_string()),
    };
    save_session(&session_state).unwrap();
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let terminal_result = SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary: "status: 400".to_string(),
        tool: "gemini-cli".to_string(),
        ..make_result("failure", 1)
    };
    save_result(project, &session_id, &terminal_result).unwrap();
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 1\nstatus = \"failure\"\n",
    )
    .unwrap();
    std::fs::write(
        session_dir.join("stderr.log"),
        "fallback codex review is being scheduled\n",
    )
    .unwrap();

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
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
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .unwrap();

    assert_eq!(
        exit_code, 1,
        "last-candidate Gemini failure must return the terminal result exit code"
    );
    assert_eq!(
        emitted_completion,
        Some((session_id, "failure".to_string(), 1, false)),
        "last-candidate Gemini failure must emit terminal wait completion"
    );
}
