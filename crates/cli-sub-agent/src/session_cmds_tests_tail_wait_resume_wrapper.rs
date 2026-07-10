use super::*;
use crate::session_cmds_daemon::{
    SESSION_WAIT_MEMORY_WARN_EXIT_CODE, WaitBehavior, WaitCallerIdentity, WaitLoopTiming,
    WaitReconciliationOutcome, handle_session_wait_with_hooks,
    handle_session_wait_with_hooks_and_sampler, handle_session_wait_with_identity_for_test,
    parent_pid, process_start_time_ticks, process_state, render_wait_result_summary,
    try_acquire_session_wait_lock, try_acquire_session_wait_lock_with_caller,
};
use std::fs;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

const FIX_FINDING_TASK_TYPE: &str = "review_fix_finding";

#[cfg(target_os = "linux")]
struct ForkedChildGuard(libc::pid_t);

#[cfg(target_os = "linux")]
impl ForkedChildGuard {
    fn try_wait_nohang(&mut self) -> libc::pid_t {
        let mut status: libc::c_int = 0;
        // SAFETY: self.0 is a child PID owned by this guard, status points to
        // writable storage, and WNOHANG makes the ownership check non-blocking.
        let rc = unsafe { libc::waitpid(self.0, &mut status, libc::WNOHANG) };
        let ownership_released = rc == self.0
            || (rc == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD));
        if ownership_released {
            self.0 = 0;
        }
        rc
    }
}

#[cfg(target_os = "linux")]
impl Drop for ForkedChildGuard {
    fn drop(&mut self) {
        if self.0 == 0 {
            return;
        }
        // SAFETY: self.0 is a child PID returned by fork(); kill targets only
        // that child, and waitpid receives a valid status pointer with EINTR
        // retried until the child is reaped or is already gone.
        unsafe {
            libc::kill(self.0, libc::SIGKILL);
            loop {
                let mut status: libc::c_int = 0;
                let rc = libc::waitpid(self.0, &mut status, 0);
                if rc == self.0 {
                    break;
                }
                if rc == -1 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    break;
                }
            }
        }
    }
}

#[path = "session_cmds_tests_tail_wait_resume_wrapper_alias_assert.rs"]
mod alias_assert;
#[path = "session_cmds_tests_tail_wait_resume_wrapper_alias_race.rs"]
mod alias_race;

fn create_session(
    project_path: &Path,
    description: Option<&str>,
    parent_id: Option<&str>,
    tool: Option<&str>,
) -> anyhow::Result<csa_session::MetaSessionState> {
    csa_session::create_session_fresh(project_path, description, parent_id, tool)
}

fn run_git(project_root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_git_repo(project_root: &Path) {
    run_git(project_root, &["init", "-q"]);
    run_git(
        project_root,
        &["config", "user.email", "csa-test@example.com"],
    );
    run_git(project_root, &["config", "user.name", "CSA Test"]);
    run_git(project_root, &["config", "commit.gpgsign", "false"]);
    fs::write(project_root.join("tracked.txt"), "initial\n").unwrap();
    run_git(project_root, &["add", "tracked.txt"]);
    run_git(project_root, &["commit", "-q", "-m", "initial"]);
}

fn move_session_to_legacy_root(project: &Path, session_id: &str) -> PathBuf {
    let primary_root = csa_session::get_session_root(project).unwrap();
    let primary_session_dir = primary_root.join("sessions").join(session_id);
    let primary_state_dir = csa_config::paths::state_dir_write().unwrap();
    let legacy_state_dir = csa_config::paths::legacy_state_dir().unwrap();
    let relative_root = primary_root.strip_prefix(&primary_state_dir).unwrap();
    let legacy_root = legacy_state_dir.join(relative_root);
    let legacy_sessions_dir = legacy_root.join("sessions");
    fs::create_dir_all(&legacy_sessions_dir).unwrap();
    let legacy_session_dir = legacy_sessions_dir.join(session_id);
    fs::rename(&primary_session_dir, &legacy_session_dir).unwrap();
    legacy_session_dir
}

fn set_tree_file_mtimes_seconds_ago(path: &Path, seconds_ago: u64) {
    let stale_time = SystemTime::now()
        .checked_sub(Duration::from_secs(seconds_ago))
        .unwrap();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
                file.set_times(fs::FileTimes::new().set_modified(stale_time))
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
    fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let worker = create_session(project, Some("worker"), None, Some("codex")).unwrap();
    let wrapper = create_session(project, Some("wrapper"), None, None).unwrap();
    let worker_id = worker.meta_session_id;
    let wrapper_id = wrapper.meta_session_id;
    let wrapper_dir = get_session_dir(project, &wrapper_id).unwrap();
    csa_session::write_resume_target(project, &wrapper_id, &worker_id).unwrap();
    fs::write(
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
    fs::create_dir_all(&state_home).unwrap();
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
    fs::write(
        wrapper_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .unwrap();
    set_tree_file_mtimes_seconds_ago(&wrapper_dir, 120);
    fs::write(
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
                poll_interval: Duration::from_millis(1),
                memory_sample_interval: Duration::from_secs(15),
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
    fs::create_dir_all(&state_home).unwrap();
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
    let _worktree_lock = csa_lock::acquire_worktree_write_lock(
        project,
        &wrapper_id,
        &[],
        |_| false,
        |_| false,
        |_| false,
    )
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
    fs::create_dir_all(&state_home).unwrap();
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
    fs::write(
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
                poll_interval: Duration::from_millis(1),
                memory_sample_interval: Duration::ZERO,
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
    fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let worker = create_session(project, Some("worker-no-result"), None, Some("codex")).unwrap();
    let wrapper = create_session(project, Some("wrapper-completed"), None, None).unwrap();
    let worker_id = worker.meta_session_id;
    let wrapper_id = wrapper.meta_session_id;
    let wrapper_dir = get_session_dir(project, &wrapper_id).unwrap();
    csa_session::write_resume_target(project, &wrapper_id, &worker_id).unwrap();
    fs::write(
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
fn handle_session_wait_on_fix_finding_wrapper_reports_fix_session_missing_result() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    fs::create_dir_all(&state_home).unwrap();
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
    assert_ne!(fix_session_id, original_review_id);

    fs::write(project.join("tracked.txt"), "fixed but not recorded\n").unwrap();

    let wrapper =
        csa_session::create_session_fresh(project, Some("fix-finding wrapper"), None, None)
            .unwrap();
    let wrapper_id = wrapper.meta_session_id;
    assert_ne!(wrapper_id, original_review_id);
    assert_ne!(wrapper_id, fix_session_id);
    let wrapper_dir = get_session_dir(project, &wrapper_id).unwrap();
    csa_session::write_resume_target(project, &wrapper_id, &fix_session_id).unwrap();
    assert_eq!(
        csa_session::read_resume_target_from_dir(&wrapper_dir).unwrap(),
        Some(fix_session_id.clone())
    );
    fs::write(
        wrapper_dir.join("daemon-completion.toml"),
        "exit_code = 1\nstatus = \"failure\"\n",
    )
    .unwrap();

    let exit_code = handle_session_wait(
        wrapper_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        1,
    )
    .unwrap();

    assert_eq!(exit_code, 1);
    assert!(
        load_result(project, &wrapper_id).unwrap().is_none(),
        "wrapper must stay an alias and must not get the fix result"
    );
    let original_result = load_result(project, &original_review_id)
        .unwrap()
        .expect("original review result remains present");
    assert_eq!(original_result.status, "failure");

    let fix_result = load_result(project, &fix_session_id)
        .unwrap()
        .expect("fix session should get synthetic diagnostic result");
    let fix_dir = get_session_dir(project, &fix_session_id).unwrap();
    let wrapper_summary = render_wait_result_summary(&fix_dir, &wrapper_id, &fix_result);
    alias_assert::assert_fix_finding_wrapper_summary(
        &wrapper_summary,
        &wrapper_id,
        &fix_session_id,
        &original_review_id,
    );

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

#[cfg(unix)]
#[test]
fn handle_session_wait_on_resume_wrapper_follows_worker_in_legacy_root() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    fs::create_dir_all(&state_home).unwrap();
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
    fs::write(
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
    fs::create_dir_all(&state_home).unwrap();
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
    fs::write(
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

#[cfg(unix)]
#[test]
fn wait_lock_stale_pid_diagnostic_with_free_flock_is_reclaimed() {
    use std::io::Write;

    let td = tempdir().unwrap();
    let session_dir = td.path();

    // Write a stale diagnostic with a definitely-dead PID.
    // No process holds the flock — the kernel released it when the dead process exited.
    let stale_json = serde_json::json!({"pid": i32::MAX, "pid_start_time_ticks": null});
    let mut f = std::fs::File::create(session_dir.join(".wait.lock")).unwrap();
    writeln!(f, "{stale_json}").unwrap();
    drop(f);

    let lock = try_acquire_session_wait_lock(session_dir)
        .expect("acquire should not error")
        .expect("dead PID diagnostic with free flock should be reclaimed");

    // Verify the diagnostic was updated with the current PID.
    let content = std::fs::read_to_string(session_dir.join(".wait.lock")).unwrap();
    let diag: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(diag["pid"].as_u64(), Some(u64::from(std::process::id())));

    drop(lock);
}

#[cfg(unix)]
#[test]
fn wait_lock_dead_pid_with_held_flock_does_not_steal() {
    use std::io::Write;
    use std::os::fd::AsRawFd;

    let td = tempdir().unwrap();
    let session_dir = td.path();

    // Write a stale diagnostic with a dead PID.
    let stale_json = serde_json::json!({"pid": i32::MAX, "pid_start_time_ticks": null});
    let mut f = std::fs::File::create(session_dir.join(".wait.lock")).unwrap();
    writeln!(f, "{stale_json}").unwrap();
    drop(f);

    // A live process holds the flock now.
    let live_flock = std::fs::OpenOptions::new()
        .write(true)
        .open(session_dir.join(".wait.lock"))
        .unwrap();
    // SAFETY: live_flock owns a valid fd; LOCK_EX | LOCK_NB is a non-blocking advisory lock.
    unsafe {
        libc::flock(live_flock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB);
    }

    let result = try_acquire_session_wait_lock(session_dir).expect("acquire should not error");
    assert!(
        result.is_none(),
        "must not steal wait lock held by live flock even if diagnostic PID is dead"
    );
}

#[cfg(unix)]
#[test]
fn wait_lock_fd_sets_cloexec() {
    let td = tempdir().unwrap();
    let session_dir = td.path();

    let lock = try_acquire_session_wait_lock(session_dir)
        .expect("acquire should not error")
        .expect("should acquire on fresh lock");

    // SAFETY: read-only F_GETFD on a valid fd.
    let flags = unsafe { libc::fcntl(lock.raw_fd(), libc::F_GETFD) };
    assert!(
        flags & libc::FD_CLOEXEC != 0,
        "wait lock fd must have FD_CLOEXEC set"
    );

    drop(lock);
}

#[cfg(target_os = "linux")]
#[test]
fn wait_lock_orphaned_pid_not_reclaimed_when_parent_alive() {
    // A live wait process with a matching parent PID should NOT be reclaimed.
    // We verify that when our own PID (alive, parent unchanged) holds the flock,
    // a second acquire correctly fails — the orphan check doesn't fire.
    use std::io::Write;
    use std::os::fd::AsRawFd;

    let td = tempdir().unwrap();
    let session_dir = td.path();

    // Write diagnostic with our PID and a valid start-time.
    let my_pid = std::process::id();
    let my_start_time =
        process_start_time_ticks(my_pid).expect("Linux test process should expose a start time");
    let my_ppid = parent_pid(my_pid).expect("Linux test process should expose a parent PID");
    let diag_json = serde_json::json!({"pid": my_pid, "pid_start_time_ticks": my_start_time, "parent_pid": my_ppid});
    let mut f = std::fs::File::create(session_dir.join(".wait.lock")).unwrap();
    writeln!(f, "{diag_json}").unwrap();
    drop(f);

    // Hold the flock ourselves (alive, parent PID unchanged).
    let live_flock = std::fs::OpenOptions::new()
        .write(true)
        .open(session_dir.join(".wait.lock"))
        .unwrap();
    // SAFETY: live_flock owns a valid fd; LOCK_EX | LOCK_NB is a non-blocking advisory lock.
    let flock_rc = unsafe { libc::flock(live_flock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(
        flock_rc,
        0,
        "test holder should acquire flock: {}",
        std::io::Error::last_os_error()
    );

    let result = try_acquire_session_wait_lock(session_dir).expect("acquire should not error");
    assert!(
        result.is_none(),
        "must not reclaim lock held by live process with a live parent"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn wait_lock_rewritten_diagnostic_cannot_signal_non_holder() {
    let td = tempdir().unwrap();
    let lock_path = td.path().join(".wait.lock");

    // Fork the diagnostic victim before opening the lock so it cannot inherit
    // the locked open-file description.
    // SAFETY: the child branch calls only async-signal-safe libc functions
    // before _exit().
    let victim_pid = unsafe { libc::fork() };
    assert!(victim_pid >= 0, "fork failed");
    if victim_pid == 0 {
        // SAFETY: pause() and _exit() are async-signal-safe after fork().
        unsafe {
            libc::pause();
            libc::_exit(0);
        }
    }
    let mut victim_guard = ForkedChildGuard(victim_pid);

    let victim_start_time = process_start_time_ticks(victim_pid as u32)
        .expect("diagnostic victim should expose a start time");
    let victim_parent_pid =
        parent_pid(victim_pid as u32).expect("diagnostic victim should expose its parent PID");
    let different_parent_pid = victim_parent_pid
        .checked_add(1)
        .unwrap_or(victim_parent_pid - 1);

    let holder = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    // SAFETY: holder owns a valid fd; LOCK_EX | LOCK_NB requests an exclusive
    // non-blocking advisory flock.
    assert_eq!(
        unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0,
        "test holder should acquire flock"
    );

    // Advisory flock does not prevent this concurrent diagnostic rewrite. The
    // diagnostic names a live reparented-looking process that is not the holder.
    let diagnostic = serde_json::json!({
        "pid": victim_pid,
        "pid_start_time_ticks": victim_start_time,
        "parent_pid": different_parent_pid,
    });
    std::fs::write(&lock_path, format!("{diagnostic}\n")).unwrap();

    let acquired = try_acquire_session_wait_lock(td.path()).expect("lock probe should not error");
    assert!(
        acquired.is_none(),
        "rewritten diagnostics must not bypass the holder's flock"
    );

    let victim_wait = victim_guard.try_wait_nohang();
    assert_eq!(
        victim_wait, 0,
        "takeover must preserve the diagnostic PID when it does not hold the flock"
    );

    drop(holder);
}

#[cfg(target_os = "linux")]
#[test]
fn wait_lock_diagnostic_probe_does_not_reap_unowned_child() {
    let td = tempdir().unwrap();
    let lock_path = td.path().join(".wait.lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    // SAFETY: lock_file owns a valid fd and the flags request a non-blocking
    // exclusive advisory lock before fork().
    assert_eq!(
        unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0,
        "parent should acquire flock before forking its holder"
    );

    // SAFETY: the child branch calls only async-signal-safe libc functions
    // before _exit(); it inherits the locked open-file description.
    let holder_pid = unsafe { libc::fork() };
    assert!(holder_pid >= 0, "holder fork failed");
    if holder_pid == 0 {
        // SAFETY: pause() and _exit() are async-signal-safe after fork().
        unsafe {
            libc::pause();
            libc::_exit(0);
        }
    }
    let _holder_guard = ForkedChildGuard(holder_pid);
    drop(lock_file);

    // SAFETY: the child exits immediately using the async-signal-safe _exit().
    let exited_pid = unsafe { libc::fork() };
    assert!(exited_pid >= 0, "exited-child fork failed");
    if exited_pid == 0 {
        // SAFETY: _exit() is async-signal-safe after fork().
        unsafe { libc::_exit(0) };
    }
    let mut exited_guard = ForkedChildGuard(exited_pid);
    let exited_pid_u32 = exited_pid as u32;
    let exited_start_time = process_start_time_ticks(exited_pid_u32)
        .expect("exited child should expose a Linux start time until reaped");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while process_state(exited_pid_u32) != Some('Z') {
        assert!(
            std::time::Instant::now() < deadline,
            "child did not become waitable before the test deadline"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    for diagnostic_pid in [exited_pid_u32, 0] {
        let diagnostic = serde_json::json!({
            "pid": diagnostic_pid,
            "pid_start_time_ticks": exited_start_time,
            "parent_pid": 1,
        });
        std::fs::write(&lock_path, format!("{diagnostic}\n")).unwrap();
        let acquired =
            try_acquire_session_wait_lock(td.path()).expect("lock probe should not error");
        assert!(
            acquired.is_none(),
            "advisory diagnostic must not bypass another process's flock"
        );
    }

    assert_eq!(
        exited_guard.try_wait_nohang(),
        exited_pid,
        "advisory liveness probes must leave child reaping to its owner"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn wait_lock_legacy_orphaned_pid_is_reclaimed() {
    // Positive compatibility test: create a live holder whose legacy recorded
    // parent differs from its actual parent, then verify lock acquisition
    // reclaims the holder via SIGTERM.
    //
    // Design:
    // - Acquire flock BEFORE fork on a parent-opened fd. After fork the child
    //   inherits the locked open-file description. The parent closes its dup
    //   so only the child holds the lock. This avoids calling flock() after
    //   fork (not on the POSIX async-signal-safe list).
    // - RAII ChildGuard kills+reaps the child in Drop (with EINTR retry),
    //   preventing zombies or hung pause() children even if assertions fail.

    let td = tempdir().unwrap();
    let session_dir = td.path();
    let lock_path = session_dir.join(".wait.lock");

    // Open lock file and acquire flock BEFORE fork.
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    let lock_fd = std::os::unix::io::AsRawFd::as_raw_fd(&lock_file);
    // SAFETY: lock_file owns lock_fd, and the flags request a non-blocking advisory lock.
    let flock_rc = unsafe { libc::flock(lock_fd, libc::LOCK_EX | libc::LOCK_NB) };
    assert!(flock_rc == 0, "parent should acquire flock before fork");

    // Protocol byte for success (avoid b"\x00" which triggers clippy::manual-c-str-literals).
    let ok_byte: [u8; 1] = [0];

    // Create pipe for readiness handshake.
    let mut pipe_fds = [0i32; 2];
    // SAFETY: pipe_fds has space for the two file descriptors written by pipe().
    assert!(
        unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } == 0,
        "pipe failed"
    );
    let read_fd = pipe_fds[0];
    let write_fd = pipe_fds[1];

    // Fork holder child. The child inherits the locked fd.
    // SAFETY: the child branch calls only async-signal-safe libc functions before _exit().
    let child_pid = unsafe { libc::fork() };
    assert!(child_pid >= 0, "fork failed");

    if child_pid == 0 {
        // Holder child — only async-signal-safe calls from here.
        // The inherited lock_fd already holds the flock.
        // SAFETY: read_fd is the child's valid inherited read end and is no longer needed.
        unsafe { libc::close(read_fd) };

        // Signal readiness immediately (flock already held). Retry EINTR a
        // bounded number of times; the parent independently enforces the
        // overall five-second handshake deadline.
        // SAFETY: write_fd is valid, ok_byte supplies one readable byte, errno
        // is read only after a failed write, and all child syscalls used here
        // are async-signal-safe after fork().
        unsafe {
            let mut interrupted_writes = 0;
            loop {
                let rc = libc::write(write_fd, ok_byte.as_ptr().cast(), 1);
                if rc == 1 {
                    break;
                }
                let was_interrupted = rc == -1 && *libc::__errno_location() == libc::EINTR;
                if was_interrupted && interrupted_writes < 16 {
                    interrupted_writes += 1;
                    continue;
                }
                libc::_exit(1);
            }
            libc::close(write_fd);
        }

        // Wait to be SIGTERM'd by the reclaimer.
        // SAFETY: pause() and _exit() are async-signal-safe after fork().
        unsafe {
            libc::pause();
            libc::_exit(0);
        }
    }

    // --- Parent continues here ---
    // Close parent's copy of lock_fd so only the child holds the lock.
    drop(lock_file);

    // RAII guard kills and reaps the child on every parent exit path.
    let _guard = ForkedChildGuard(child_pid);

    // SAFETY: write_fd is the parent's inherited write end and is no longer needed.
    unsafe { libc::close(write_fd) };

    // Bounded pipe read: use poll() with 5s timeout to avoid infinite hang.
    // Retry poll() and read() on EINTR.
    let mut readiness = [0u8; 1];
    let nbytes: isize;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let remaining = deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_millis()
            .min(u32::MAX as u128) as libc::c_int;
        if remaining <= 0 {
            nbytes = -1;
            break;
        }
        let mut pollfd = libc::pollfd {
            fd: read_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pollfd points to one initialized descriptor entry for this call.
        let poll_rc = unsafe { libc::poll(&mut pollfd, 1, remaining) };
        if poll_rc == -1 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            nbytes = -1;
            break;
        }
        if poll_rc > 0 {
            // SAFETY: read_fd is open and readiness provides one writable byte.
            let rc = unsafe { libc::read(read_fd, readiness.as_mut_ptr() as *mut _, 1) };
            if rc == -1 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                nbytes = -1;
                break;
            }
            nbytes = rc;
            break;
        }
        // poll_rc == 0: timeout
        nbytes = -1;
        break;
    }
    // SAFETY: read_fd is the parent's valid read end and is no longer needed.
    unsafe { libc::close(read_fd) };

    assert!(nbytes == 1, "child should signal readiness within 5s");
    assert!(
        readiness[0] == ok_byte[0],
        "child should have successfully acquired flock"
    );

    // SAFETY: kill(pid, 0) probes the forked child's liveness without sending a signal.
    let alive = unsafe { libc::kill(child_pid, 0) } == 0;
    assert!(alive, "child should be alive and holding flock");

    // Derive a legacy recorded parent guaranteed to differ from the child's
    // actual parent, including when the test process is PID-namespace init.
    let child_start_time =
        process_start_time_ticks(child_pid as u32).expect("should have start-time on Linux");
    let child_parent_pid =
        parent_pid(child_pid as u32).expect("child should expose its parent PID on Linux");
    let different_parent_pid = child_parent_pid
        .checked_add(1)
        .unwrap_or(child_parent_pid - 1);
    assert_ne!(different_parent_pid, child_parent_pid);
    let diag = serde_json::json!({
        "pid": child_pid,
        "pid_start_time_ticks": child_start_time,
        "parent_pid": different_parent_pid,
    });
    std::fs::write(&lock_path, format!("{diag}\n")).unwrap();

    // Now try to acquire — should reclaim the lock via SIGTERM + retry.
    let lock = try_acquire_session_wait_lock(session_dir).expect("acquire should not error");
    assert!(lock.is_some(), "should reclaim lock from orphaned holder");

    // ChildGuard::drop will kill+reap the child.
}
