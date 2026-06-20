use super::*;

#[cfg(target_os = "linux")]
fn read_process_start_time_ticks(pid: u32) -> u64 {
    let stat_path = format!("/proc/{pid}/stat");
    let content = std::fs::read_to_string(stat_path).expect("read process stat");
    let close_paren = content.rfind(')').expect("process stat comm terminator");
    let after_comm = &content[close_paren + 1..];
    let mut parts = after_comm.split_whitespace();
    parts.next().expect("process state");
    parts.next().expect("parent pid");
    parts.next().expect("process group");
    for _ in 0..16 {
        parts.next().expect("intermediate stat field");
    }
    parts
        .next()
        .expect("process start time")
        .parse::<u64>()
        .expect("parse process start time")
}

#[cfg(target_os = "linux")]
fn daemon_pid_record(pid: u32) -> String {
    format!("{pid} {}\n", read_process_start_time_ticks(pid))
}

fn wait_until_wait_lock_is_held(session_dir: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match try_acquire_session_wait_lock(session_dir).expect("probe wait lock") {
            Some(lock) => {
                drop(lock);
                assert!(
                    std::time::Instant::now() < deadline,
                    "session wait did not rebind to target wait lock at {}",
                    session_dir.display()
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            None => return,
        }
    }
}

#[cfg(unix)]
#[test]
fn handle_session_wait_on_fix_finding_wrapper_rebinds_when_alias_appears_after_wait_starts() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();
    init_git_repo(project);

    let original_review = csa_session::create_session_fresh(
        project,
        Some("original failed review"),
        None,
        Some("codex"),
    )
    .unwrap();
    let original_review_id = original_review.meta_session_id;
    save_result(project, &original_review_id, &make_result("failure", 1)).unwrap();

    let wrapper =
        csa_session::create_session_fresh(project, Some("fix-finding wrapper"), None, None)
            .unwrap();
    let wrapper_id = wrapper.meta_session_id;
    let wrapper_dir = get_session_dir(project, &wrapper_id).unwrap();
    std::fs::write(
        wrapper_dir.join("stderr.log"),
        "wrapper bootstrap is still preparing the fix session\n",
    )
    .unwrap();
    assert!(
        csa_process::ToolLiveness::is_alive(&wrapper_dir),
        "test setup requires the pre-alias wrapper to look alive"
    );

    let wait_project_arg = project.to_string_lossy().into_owned();
    let wait_wrapper_id = wrapper_id.clone();
    let wait_handle = std::thread::spawn(move || {
        handle_session_wait(wait_wrapper_id, Some(wait_project_arg), 5)
            .expect("wait should complete after alias and completion appear")
    });
    std::thread::sleep(std::time::Duration::from_millis(50));

    let mut fix_session = csa_session::create_session_fresh(
        project,
        Some("fix finding from review"),
        Some(&original_review_id),
        Some("codex"),
    )
    .unwrap();
    fix_session.phase = SessionPhase::Active;
    fix_session.task_context = TaskContext {
        task_type: Some(FIX_FINDING_TASK_TYPE.to_string()),
        tier_name: None,
    };
    save_session(&fix_session).unwrap();
    let fix_session_id = fix_session.meta_session_id;
    let fix_dir = get_session_dir(project, &fix_session_id).unwrap();
    assert_ne!(fix_session_id, original_review_id);
    assert_ne!(wrapper_id, original_review_id);
    assert_ne!(wrapper_id, fix_session_id);

    std::fs::write(project.join("tracked.txt"), "fixed but not recorded\n").unwrap();
    csa_session::write_resume_target(project, &wrapper_id, &fix_session_id).unwrap();
    assert_eq!(
        csa_session::read_resume_target_from_dir(&wrapper_dir).unwrap(),
        Some(fix_session_id.clone())
    );
    wait_until_wait_lock_is_held(&fix_dir);
    std::fs::write(
        wrapper_dir.join("daemon-completion.toml"),
        "exit_code = 1\nstatus = \"failure\"\n",
    )
    .unwrap();

    let exit_code = wait_handle.join().expect("wait thread should not panic");

    assert_eq!(exit_code, 1);
    assert!(
        load_result(project, &wrapper_id).unwrap().is_none(),
        "late alias must keep wrapper as an alias and must not get the fix result"
    );
    let fix_result = load_result(project, &fix_session_id)
        .unwrap()
        .expect("late alias should route diagnostics to the real fix session");
    let summary = render_wait_result_summary(&fix_dir, &fix_session_id, &fix_result);

    assert!(
        summary.contains(&format!("Session: {fix_session_id}")),
        "{summary}"
    );
    assert!(summary.contains("fix-finding"), "{summary}");
    assert!(
        summary.contains("original failed review verdict is not a fix-session result"),
        "{summary}"
    );
    assert!(
        summary.contains("repo_side_effects=dirty_or_committed_tracked_changes"),
        "{summary}"
    );
    assert!(summary.contains("modified=[tracked.txt]"), "{summary}");
    assert!(
        !summary.contains(&format!("Session: {original_review_id}")),
        "{summary}"
    );
    assert!(!summary.contains("Review verdict: FAIL"), "{summary}");
}

#[cfg(target_os = "linux")]
#[test]
fn handle_session_wait_on_resume_wrapper_defers_target_reconcile_while_wrapper_daemon_alive() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let mut target = create_session(
        project,
        Some("target-before-bootstrap-liveness"),
        None,
        Some("codex"),
    )
    .unwrap();
    target.phase = SessionPhase::Active;
    let target_id = target.meta_session_id.clone();
    save_session(&target).unwrap();
    let target_dir = get_session_dir(project, &target_id).unwrap();
    set_tree_file_mtimes_seconds_ago(&target_dir, 120);

    let wrapper =
        create_session(project, Some("live-wrapper-before-target-lock"), None, None).unwrap();
    let wrapper_id = wrapper.meta_session_id;
    let wrapper_dir = get_session_dir(project, &wrapper_id).unwrap();
    csa_session::write_resume_target(project, &wrapper_id, &target_id).unwrap();

    let mut child = Command::new("sleep").arg("5").spawn().unwrap();
    std::fs::write(
        wrapper_dir.join("daemon.pid"),
        daemon_pid_record(child.id()),
    )
    .unwrap();
    assert!(
        csa_process::ToolLiveness::daemon_pid_is_alive(&wrapper_dir),
        "test setup requires a live wrapper daemon.pid"
    );
    assert!(
        !target_dir
            .join(csa_session::result::RESULT_FILE_NAME)
            .exists(),
        "target result must be absent before bootstrap liveness appears"
    );

    let mut reconcile_called = false;
    let exit_code = handle_session_wait_with_hooks(
        wrapper_id,
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
            reconcile_called = true;
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |_sid: &str, _status: &str, _exit_code, _synthetic, _mirror_to_stdout| {
            panic!("live wrapper handoff must not emit target terminal completion")
        },
    )
    .unwrap();

    child.kill().ok();
    let _ = child.wait();

    assert_eq!(
        exit_code, 0,
        "live wrapper handoff should take the healthy wait-cap path"
    );
    assert!(
        !reconcile_called,
        "target reconcile must be deferred while the wrapper daemon still owns bootstrap"
    );
    assert!(
        load_result(project, &target_id).unwrap().is_none(),
        "target must not receive synthetic failure before it has bootstrap liveness"
    );
}
