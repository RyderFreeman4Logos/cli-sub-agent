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
fn handle_session_wait_does_not_terminalize_gemini_failure_during_tier_fallback_handoff() {
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
    save_result(
        project,
        &session_id,
        &SessionResult {
            status: "failure".to_string(),
            exit_code: 1,
            summary: "status: 400".to_string(),
            tool: "gemini-cli".to_string(),
            ..make_result("failure", 1)
        },
    )
    .unwrap();
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
        "Gemini failure must stay nonterminal while tier fallback handoff is live"
    );
    assert!(
        !emitted_completion,
        "pending fallback handoff must not emit terminal wait completion"
    );
}
