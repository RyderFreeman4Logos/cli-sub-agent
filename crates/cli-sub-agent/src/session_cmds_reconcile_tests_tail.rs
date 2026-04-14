use super::*;
use crate::test_env_lock::TEST_ENV_LOCK;
#[cfg(unix)]
use chrono::Utc;
use csa_session::{create_session, get_session_dir, load_result};
#[cfg(unix)]
use csa_session::{load_session, save_result};
#[cfg(unix)]
use std::ffi::CString;
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
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

struct SessionTestEnv {
    _env_lock: std::sync::MutexGuard<'static, ()>,
    _home_guard: EnvVarGuard,
    _state_guard: EnvVarGuard,
}

impl SessionTestEnv {
    fn new(td: &tempfile::TempDir) -> Self {
        let env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
        let state_home = td.path().join("xdg-state");
        fs::create_dir_all(&state_home).expect("create state home");
        let home_guard = EnvVarGuard::set("HOME", td.path());
        let state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
        Self {
            _env_lock: env_lock,
            _home_guard: home_guard,
            _state_guard: state_guard,
        }
    }
}

fn run_git(dir: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn tail_set_file_mtime_seconds_ago(path: &std::path::Path, seconds_ago: u64) {
    let target = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(seconds_ago))
        .expect("target time before unix epoch")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch");
    let tv_sec = libc::time_t::try_from(target.as_secs()).expect("mtime seconds fit in time_t");
    let tv_nsec = target.subsec_nanos() as libc::c_long;
    let times = [
        libc::timespec { tv_sec, tv_nsec },
        libc::timespec { tv_sec, tv_nsec },
    ];
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("path contains NUL");
    // SAFETY: `utimensat` receives a valid C path pointer and valid timespec array.
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
    assert_eq!(rc, 0, "utimensat failed for {}", path.display());
}

#[cfg(unix)]
fn tail_backdate_tree(path: &std::path::Path, seconds_ago: u64) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).expect("read_dir") {
            let entry = entry.expect("dir entry");
            tail_backdate_tree(&entry.path(), seconds_ago);
        }
    }
    tail_set_file_mtime_seconds_ago(path, seconds_ago);
}

#[test]
fn ensure_terminal_result_for_dead_active_session_writes_unpushed_commit_sidecar() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path().join("project");
    let origin = td.path().join("origin.git");
    fs::create_dir_all(&project).unwrap();

    run_git(&project, &["init", "--initial-branch", "main"]);
    run_git(&project, &["config", "user.email", "test@example.com"]);
    run_git(&project, &["config", "user.name", "Test User"]);
    fs::write(project.join("README.md"), "base\n").unwrap();
    run_git(&project, &["add", "README.md"]);
    run_git(&project, &["commit", "-m", "init"]);

    run_git(td.path(), &["init", "--bare", origin.to_str().unwrap()]);
    run_git(
        &project,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    run_git(&project, &["push", "-u", "origin", "main"]);

    run_git(&project, &["checkout", "-b", "fix/session-progress"]);
    fs::write(project.join("progress-1.txt"), "first\n").unwrap();
    run_git(&project, &["add", "progress-1.txt"]);
    run_git(&project, &["commit", "-m", "feat: first progress"]);
    fs::write(project.join("progress-2.txt"), "second\n").unwrap();
    run_git(&project, &["add", "progress-2.txt"]);
    run_git(&project, &["commit", "-m", "fix: second progress"]);

    let session = create_session(&project, Some("unpushed-progress"), None, Some("codex")).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(&project, &session_id).unwrap();
    tail_backdate_tree(&session_dir, 120);

    let reconciled =
        ensure_terminal_result_for_dead_active_session(&project, &session_id, "session wait")
            .unwrap();
    assert_eq!(
        reconciled,
        DeadActiveSessionReconciliation::SynthesizedFailure
    );

    let sidecar_path = session_dir.join("output").join("unpushed_commits.json");
    let sidecar: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    assert_eq!(sidecar["branch"], "fix/session-progress");
    assert_eq!(sidecar["commits_ahead"], 2);
    assert_eq!(
        sidecar["recovery_command"],
        "git push -u origin fix/session-progress"
    );
    assert_eq!(sidecar["commits"].as_array().unwrap().len(), 2);
    assert!(
        sidecar["commits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["subject"] == "feat: first progress")
    );
    assert!(
        sidecar["commits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["subject"] == "fix: second progress")
    );

    let result = load_result(&project, &session_id).unwrap().unwrap();
    assert!(
        result
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/unpushed_commits.json"),
        "synthetic result should advertise the recovery sidecar"
    );
}

#[cfg(unix)]
#[test]
fn retire_if_dead_with_result_preserves_pr_bot_handoff_for_real_results() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("late-real-result"), None, Some("codex")).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    tail_backdate_tree(&session_dir, 120);

    let now = Utc::now();
    save_result(
        project,
        &session_id,
        &csa_session::SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "real terminal result".to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
        },
    )
    .unwrap();

    csa_session::write_review_meta(
        &session_dir,
        &csa_session::ReviewSessionMeta {
            session_id: session_id.clone(),
            head_sha: "deadbeef".to_string(),
            decision: "pass".to_string(),
            verdict: "CLEAN".to_string(),
            tool: "codex".to_string(),
            scope: "range:main...HEAD".to_string(),
            exit_code: 0,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 1,
            timestamp: now,
            diff_fingerprint: None,
        },
    )
    .unwrap();

    let retired = retire_if_dead_with_result_impl(
        project,
        &session_id,
        "session wait",
        &session_dir,
        &persist_session_state_atomically,
    )
    .unwrap();
    assert!(retired);
    assert!(
        !session_dir
            .join("output")
            .join("unpushed_commits.json")
            .exists(),
        "real-result retirement must not leave an unpushed-commit recovery sidecar"
    );

    let directive = crate::session_cmds_daemon::synthesized_wait_next_step(&session_dir)
        .unwrap()
        .expect("cumulative review should still synthesize a next-step directive");
    assert!(directive.contains("CSA:NEXT_STEP"));
    assert!(directive.contains("pr-bot"));

    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(persisted.phase, csa_session::SessionPhase::Retired);
    assert_eq!(persisted.termination_reason.as_deref(), Some("completed"));
}

#[test]
fn inspect_unpushed_commits_uses_session_branch_not_checked_out_head() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path().join("project");
    let origin = td.path().join("origin.git");
    fs::create_dir_all(&project).unwrap();

    run_git(&project, &["init", "--initial-branch", "main"]);
    run_git(&project, &["config", "user.email", "test@example.com"]);
    run_git(&project, &["config", "user.name", "Test User"]);
    fs::write(project.join("README.md"), "base\n").unwrap();
    run_git(&project, &["add", "README.md"]);
    run_git(&project, &["commit", "-m", "init"]);

    run_git(td.path(), &["init", "--bare", origin.to_str().unwrap()]);
    run_git(
        &project,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    run_git(&project, &["push", "-u", "origin", "main"]);

    run_git(&project, &["checkout", "-b", "fix/session-progress"]);
    fs::write(project.join("progress-1.txt"), "first\n").unwrap();
    run_git(&project, &["add", "progress-1.txt"]);
    run_git(&project, &["commit", "-m", "feat: first progress"]);
    fs::write(project.join("progress-2.txt"), "second\n").unwrap();
    run_git(&project, &["add", "progress-2.txt"]);
    run_git(&project, &["commit", "-m", "fix: second progress"]);

    let session = create_session(&project, Some("unpushed-progress"), None, Some("codex")).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(&project, &session_id).unwrap();
    tail_backdate_tree(&session_dir, 120);

    run_git(&project, &["checkout", "main"]);
    run_git(&project, &["checkout", "-b", "operator/other-work"]);

    let reconciled =
        ensure_terminal_result_for_dead_active_session(&project, &session_id, "session wait")
            .unwrap();
    assert_eq!(
        reconciled,
        DeadActiveSessionReconciliation::SynthesizedFailure
    );

    let sidecar_path = session_dir.join("output").join("unpushed_commits.json");
    let sidecar: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    assert_eq!(sidecar["branch"], "fix/session-progress");
    assert_eq!(sidecar["commits_ahead"], 2);
    assert_eq!(
        sidecar["recovery_command"],
        "git push -u origin fix/session-progress"
    );
    assert_eq!(sidecar["commits"].as_array().unwrap().len(), 2);
    assert!(
        sidecar["commits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["subject"] == "feat: first progress")
    );
    assert!(
        sidecar["commits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["subject"] == "fix: second progress")
    );
}
