use super::*;
use crate::session_cmds_daemon::{
    SESSION_WAIT_MEMORY_WARN_EXIT_CODE, WaitBehavior, WaitLoopTiming, WaitReconciliationOutcome,
    handle_session_wait_with_hooks, handle_session_wait_with_hooks_and_sampler,
    try_acquire_session_wait_lock,
};

fn move_session_to_legacy_root(project: &std::path::Path, session_id: &str) -> std::path::PathBuf {
    let primary_root = csa_session::get_session_root(project).unwrap();
    let primary_session_dir = primary_root.join("sessions").join(session_id);
    let primary_state_dir = csa_config::paths::state_dir_write().unwrap();
    let legacy_state_dir = csa_config::paths::legacy_state_dir().unwrap();
    let relative_root = primary_root.strip_prefix(&primary_state_dir).unwrap();
    let legacy_root = legacy_state_dir.join(relative_root);
    let legacy_sessions_dir = legacy_root.join("sessions");
    std::fs::create_dir_all(&legacy_sessions_dir).unwrap();
    let legacy_session_dir = legacy_sessions_dir.join(session_id);
    std::fs::rename(&primary_session_dir, &legacy_session_dir).unwrap();
    legacy_session_dir
}

fn set_tree_file_mtimes_seconds_ago(path: &std::path::Path, seconds_ago: u64) {
    let stale_time = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(seconds_ago))
        .unwrap();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
                file.set_times(std::fs::FileTimes::new().set_modified(stale_time))
                    .unwrap();
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn handle_session_wait_on_resume_wrapper_uses_worker_result() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let worker = create_session(project, Some("worker"), None, Some("codex")).unwrap();
    let wrapper = create_session(project, Some("wrapper"), None, None).unwrap();
    let worker_id = worker.meta_session_id;
    let wrapper_id = wrapper.meta_session_id;
    let wrapper_dir = get_session_dir(project, &wrapper_id).unwrap();
    csa_session::write_resume_target(project, &wrapper_id, &worker_id).unwrap();
    std::fs::write(
        wrapper_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .unwrap();
    save_result(project, &worker_id, &make_result("success", 0)).unwrap();

    let exit_code = handle_session_wait(
        wrapper_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        1,
    )
    .unwrap();

    assert_eq!(exit_code, 0);
    assert!(
        load_result(project, &wrapper_id).unwrap().is_none(),
        "waiting on a resume wrapper must not synthesize or clobber wrapper result.toml"
    );
    let worker_result = load_result(project, &worker_id)
        .unwrap()
        .expect("worker result should remain authoritative");
    assert_eq!(worker_result.status, "success");
    assert_eq!(worker_result.exit_code, 0);
}

#[cfg(unix)]
#[test]
fn handle_session_wait_on_resume_wrapper_continues_while_worker_target_alive_without_result() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let worker =
        create_session(project, Some("worker-live-no-result"), None, Some("codex")).unwrap();
    let wrapper =
        create_session(project, Some("wrapper-completed-live-target"), None, None).unwrap();
    let worker_id = worker.meta_session_id;
    let wrapper_id = wrapper.meta_session_id;
    let worker_dir = get_session_dir(project, &worker_id).unwrap();
    let wrapper_dir = get_session_dir(project, &wrapper_id).unwrap();
    csa_session::write_resume_target(project, &wrapper_id, &worker_id).unwrap();
    std::fs::write(
        wrapper_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .unwrap();
    set_tree_file_mtimes_seconds_ago(&wrapper_dir, 120);
    std::fs::write(
        worker_dir.join("stderr.log"),
        "worker target still producing diagnostics\n",
    )
    .unwrap();
    assert!(
        csa_process::ToolLiveness::is_alive(&worker_dir),
        "test setup requires worker target liveness"
    );
    assert!(
        !csa_process::ToolLiveness::is_alive(&wrapper_dir),
        "test setup requires wrapper-only liveness to be dead"
    );
    assert!(
        load_result(project, &worker_id).unwrap().is_none(),
        "worker target result must be absent"
    );

    let mut reconciled_session_id: Option<String> = None;
    let mut emitted_completion = false;
    let exit_code = handle_session_wait_with_hooks(
        wrapper_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 0,
            memory_warn_mb: None,
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(1),
                memory_sample_interval: std::time::Duration::from_secs(15),
            },
        },
        |_project_root, current_session_id, trigger| {
            assert_eq!(trigger, "session wait");
            reconciled_session_id = Some(current_session_id.to_string());
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
        "live worker target should keep wrapper wait in the nonterminal KV-warm path"
    );
    assert_eq!(reconciled_session_id, Some(worker_id.clone()));
    assert!(
        !emitted_completion,
        "worker liveness without a result must not emit terminal completion"
    );
    assert!(
        load_result(project, &wrapper_id).unwrap().is_none(),
        "wrapper wait must not synthesize wrapper result.toml"
    );
    assert!(
        load_result(project, &worker_id).unwrap().is_none(),
        "worker result must remain absent while target is only live"
    );
}

#[cfg(unix)]
#[test]
fn handle_session_wait_on_resume_wrapper_treats_wrapper_worktree_lock_as_live_in_stale_precheck() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let mut worker = create_session(
        project,
        Some("worker-stale-wrapper-lock"),
        None,
        Some("codex"),
    )
    .unwrap();
    let wrapper = create_session(project, Some("wrapper-holds-lock"), None, None).unwrap();
    worker.last_accessed = chrono::Utc::now() - chrono::Duration::hours(24);
    let worker_id = worker.meta_session_id.clone();
    let wrapper_id = wrapper.meta_session_id;
    save_session(&worker).unwrap();
    csa_session::write_resume_target(project, &wrapper_id, &worker_id).unwrap();
    let _worktree_lock =
        csa_lock::acquire_worktree_write_lock(project, &wrapper_id, &[], |_| false)
            .expect("wrapper worktree lock should be held");

    let mut emitted_completion = false;
    let exit_code = handle_session_wait_with_hooks(
        wrapper_id,
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 0,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("wrapper-held worktree lock should keep stale precheck nonterminal")
        },
        |_sid: &str, _status: &str, _exit_code, _synthetic, _mirror_to_stdout| {
            emitted_completion = true;
        },
    )
    .expect("wrapper-held worktree lock should not fail stale precheck");

    assert_eq!(exit_code, 0);
    assert!(
        !emitted_completion,
        "live wrapper lock should produce a healthy wait cap, not terminal completion"
    );
}

#[cfg(unix)]
#[test]
fn handle_session_wait_on_resume_wrapper_memory_warn_samples_worker_target() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let worker = create_session(
        project,
        Some("worker-memory-warn-target"),
        None,
        Some("codex"),
    )
    .unwrap();
    let wrapper = create_session(project, Some("wrapper-memory-warn"), None, None).unwrap();
    let worker_id = worker.meta_session_id;
    let wrapper_id = wrapper.meta_session_id;
    let worker_dir = get_session_dir(project, &worker_id).unwrap();
    let wrapper_dir = get_session_dir(project, &wrapper_id).unwrap();
    csa_session::write_resume_target(project, &wrapper_id, &worker_id).unwrap();
    std::fs::write(
        worker_dir.join("stderr.log"),
        "worker target still producing diagnostics\n",
    )
    .unwrap();
    set_tree_file_mtimes_seconds_ago(&wrapper_dir, 120);
    assert!(
        csa_process::ToolLiveness::is_alive(&worker_dir),
        "test setup requires worker target liveness"
    );
    assert!(
        !csa_process::ToolLiveness::is_alive(&wrapper_dir),
        "test setup requires wrapper-only liveness to be dead"
    );

    let mut sampled_session_id: Option<String> = None;
    let mut emitted_marker: Option<(String, u64, u64)> = None;
    let exit_code = handle_session_wait_with_hooks_and_sampler(
        wrapper_id,
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
            panic!("memory warn must not emit terminal completion");
        },
        |_project_root, session_id| {
            sampled_session_id = Some(session_id.to_string());
            Ok(65)
        },
        |session_id, rss_mb, limit_mb| {
            emitted_marker = Some((session_id.to_string(), rss_mb, limit_mb));
        },
    )
    .unwrap();

    assert_eq!(exit_code, SESSION_WAIT_MEMORY_WARN_EXIT_CODE);
    assert_eq!(sampled_session_id, Some(worker_id.clone()));
    assert_eq!(emitted_marker, Some((worker_id, 65, 64)));
}

#[cfg(unix)]
#[test]
fn handle_session_wait_on_resume_wrapper_reconciles_worker_after_wrapper_completion() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let worker = create_session(project, Some("worker-no-result"), None, Some("codex")).unwrap();
    let wrapper = create_session(project, Some("wrapper-completed"), None, None).unwrap();
    let worker_id = worker.meta_session_id;
    let wrapper_id = wrapper.meta_session_id;
    let wrapper_dir = get_session_dir(project, &wrapper_id).unwrap();
    csa_session::write_resume_target(project, &wrapper_id, &worker_id).unwrap();
    std::fs::write(
        wrapper_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .unwrap();
    assert!(
        load_result(project, &worker_id).unwrap().is_none(),
        "test setup requires missing worker result.toml"
    );

    let mut reconciled_session_id: Option<String> = None;
    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        wrapper_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, current_session_id, trigger| {
            assert_eq!(trigger, "session wait");
            reconciled_session_id = Some(current_session_id.to_string());
            assert_eq!(
                current_session_id, worker_id,
                "resume wrapper wait must reconcile the worker target"
            );
            save_result(project, current_session_id, &make_result("success", 0))?;
            Ok(WaitReconciliationOutcome {
                result_became_available: true,
                synthetic: false,
            })
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .unwrap();

    assert_eq!(exit_code, 0);
    assert_eq!(reconciled_session_id, Some(worker_id.clone()));
    assert_eq!(
        emitted_completion,
        Some((wrapper_id.clone(), "success".to_string(), 0, false))
    );
    assert!(
        load_result(project, &wrapper_id).unwrap().is_none(),
        "wrapper completion must not synthesize wrapper result.toml"
    );
    assert!(
        load_result(project, &worker_id).unwrap().is_some(),
        "worker result should become authoritative"
    );
}

#[cfg(unix)]
#[test]
fn handle_session_wait_on_resume_wrapper_follows_worker_in_legacy_root() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let worker = create_session(project, Some("worker-legacy"), None, Some("codex")).unwrap();
    let wrapper = create_session(project, Some("wrapper-legacy"), None, None).unwrap();
    let worker_id = worker.meta_session_id;
    let wrapper_id = wrapper.meta_session_id;
    let wrapper_dir = get_session_dir(project, &wrapper_id).unwrap();
    let worker_dir = move_session_to_legacy_root(project, &worker_id);
    csa_session::write_resume_target(project, &wrapper_id, &worker_id)
        .expect("resume wrapper alias should accept a legacy-root target");
    std::fs::write(
        wrapper_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .unwrap();
    save_result(project, &worker_id, &make_result("success", 0)).unwrap();

    let exit_code = handle_session_wait(
        wrapper_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        1,
    )
    .unwrap();

    assert_eq!(exit_code, 0);
    assert!(
        load_result(project, &wrapper_id).unwrap().is_none(),
        "waiting on a cross-root resume wrapper must not synthesize wrapper result.toml"
    );
    assert!(
        worker_dir.join("result.toml").is_file(),
        "worker result should stay in the legacy-root session directory"
    );
}

#[cfg(unix)]
#[test]
fn handle_session_wait_on_resume_wrapper_uses_target_wait_lock() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let worker = create_session(project, Some("worker-lock"), None, Some("codex")).unwrap();
    let wrapper = create_session(project, Some("wrapper-lock"), None, None).unwrap();
    let worker_id = worker.meta_session_id;
    let wrapper_id = wrapper.meta_session_id;
    let wrapper_dir = get_session_dir(project, &wrapper_id).unwrap();
    let worker_dir = move_session_to_legacy_root(project, &worker_id);
    csa_session::write_resume_target(project, &wrapper_id, &worker_id)
        .expect("resume wrapper alias should accept a legacy-root target");
    std::fs::write(
        wrapper_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .unwrap();
    save_result(project, &worker_id, &make_result("success", 0)).unwrap();
    let _worker_wait_lock = try_acquire_session_wait_lock(&worker_dir)
        .expect("pre-acquire worker wait lock")
        .expect("worker wait lock should be acquired");

    let mut reconcile_called = false;
    let exit_code = handle_session_wait_with_hooks(
        wrapper_id,
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            reconcile_called = true;
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |_sid: &str, _status: &str, _exit_code, _synthetic, _mirror_to_stdout| {},
    )
    .unwrap();

    assert_eq!(exit_code, 1);
    assert!(
        !reconcile_called,
        "duplicate target wait lock must short-circuit"
    );
    assert!(
        !wrapper_dir.join(".wait.lock").exists(),
        "wrapper-id wait should not acquire an independent wrapper lock"
    );
}
