use super::*;
use crate::test_env_lock::TEST_ENV_LOCK;
use csa_session::{create_session, get_session_dir, load_result};
use std::fs;
use tokio::sync::OwnedMutexGuard;

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
    _env_lock: OwnedMutexGuard<()>,
    _home_guard: EnvVarGuard,
    _state_guard: EnvVarGuard,
}

impl SessionTestEnv {
    fn new(td: &tempfile::TempDir) -> Self {
        let env_lock = TEST_ENV_LOCK.clone().blocking_lock_owned();
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

#[cfg(unix)]
fn set_file_mtime_seconds_ago(path: &std::path::Path, seconds_ago: u64) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

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
fn backdate_tree(path: &std::path::Path, seconds_ago: u64) {
    if path.is_dir() {
        for entry in fs::read_dir(path).expect("read_dir") {
            let entry = entry.expect("dir entry");
            backdate_tree(&entry.path(), seconds_ago);
        }
    }
    set_file_mtime_seconds_ago(path, seconds_ago);
}

#[cfg(unix)]
#[test]
fn synthesized_failure_summary_includes_post_mortem_diagnostics() {
    let td = tempfile::tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("diagnostic-synthesis"), None, None).unwrap();
    let session_id = session.meta_session_id.clone();
    let session_dir = get_session_dir(project, &session_id).unwrap();
    fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 137\nstatus = \"failure\"\n",
    )
    .unwrap();
    fs::write(session_dir.join("daemon.pid"), "424242 1\n").unwrap();
    fs::write(
        session_dir.join("stderr.log"),
        "ACP transport failed: server shut down unexpectedly\nout of memory\n",
    )
    .unwrap();
    fs::write(
        session_dir.join("output.log"),
        "[csa-heartbeat] ACP prompt still running: elapsed=44s idle=15s\n",
    )
    .unwrap();
    fs::create_dir_all(session_dir.join("output")).unwrap();
    fs::write(
        session_dir.join("output").join("acp-events.jsonl"),
        "{\"event\":\"last\"}\n",
    )
    .unwrap();
    backdate_tree(&session_dir, 120);

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session wait")
            .unwrap();

    assert_eq!(
        reconciled,
        DeadActiveSessionReconciliation::SynthesizedFailure
    );
    let result = load_result(project, &session_id)
        .unwrap()
        .expect("synthetic result");
    assert!(result.summary.contains("Diagnostics:"));
    assert!(
        result
            .summary
            .contains("daemon_completion_packet=exit_code=137")
    );
    assert!(result.summary.contains("last_heartbeat=[csa-heartbeat]"));
    assert!(
        result
            .summary
            .contains("diagnostic_hint=possible_oom_or_sigkill")
    );
    assert!(
        result
            .summary
            .contains("acp_last_event={\"event\":\"last\"}")
    );
}
