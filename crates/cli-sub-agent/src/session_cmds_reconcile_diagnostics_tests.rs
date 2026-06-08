use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use anyhow::{Result, anyhow};
use csa_session::{
    MetaSessionState, SessionPhase, create_session, get_session_dir, load_result, load_session,
};
use std::fs;
use std::path::Path;

struct SessionTestEnv {
    _sandbox: ScopedSessionSandbox,
}

impl SessionTestEnv {
    fn new(td: &tempfile::TempDir) -> Self {
        Self {
            _sandbox: ScopedSessionSandbox::new_blocking(td),
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
    // Malformed TOML is intentional: strict packet parsing must fail so synthesis runs,
    // while lenient diagnostics still surface the exit_code hint.
    fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 137\nstatus = failure\n",
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

#[cfg(unix)]
#[test]
fn daemon_completion_packet_present_finalizes_dead_active_session() {
    let td = tempfile::tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session =
        create_session(project, Some("diagnostic-daemon-completion"), None, None).unwrap();
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
        DeadActiveSessionReconciliation::DaemonCompletionFinalized
    );
    let result = load_result(project, &session_id)
        .unwrap()
        .expect("daemon completion result");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 137);
    assert!(
        result.summary.contains("daemon completion recorded")
            || result.summary.starts_with("CSA diagnostic:"),
        "unexpected daemon completion summary: {}",
        result.summary
    );
    let raw_result = fs::read_to_string(session_dir.join("result.toml")).unwrap();
    assert!(
        raw_result.contains("kill_hint = "),
        "daemon completion result should include signal kill_hint: {raw_result}"
    );
    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(
        persisted.termination_reason.as_deref(),
        Some("daemon_completion")
    );
}

#[cfg(unix)]
#[test]
fn daemon_completion_result_survives_state_save_failure() {
    let td = tempfile::tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session =
        create_session(project, Some("daemon-completion-save-failure"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let state_path = session_dir.join("state.toml");
    let original_state = fs::read_to_string(&state_path).unwrap();
    fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .unwrap();
    let persist_fail = |_: &Path, _: &MetaSessionState| -> Result<()> { Err(anyhow!("boom")) };

    let reconciled = ensure_terminal_result_for_dead_active_session_impl(
        project,
        &session_id,
        "session wait",
        &session_dir,
        SyntheticResultHooks {
            before_write: &noop_path,
            after_publish: &noop_path,
        },
        |_| {},
        &persist_fail,
    )
    .unwrap();

    assert_eq!(
        reconciled,
        DeadActiveSessionReconciliation::DaemonCompletionFinalized
    );
    let result = load_result(project, &session_id)
        .unwrap()
        .expect("daemon completion result must remain recoverable");
    assert_eq!(result.status, "success");
    assert_eq!(result.exit_code, 0);
    assert!(
        result.summary.contains("daemon completion recorded"),
        "unexpected daemon completion summary: {}",
        result.summary
    );
    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(persisted.phase, SessionPhase::Active);
    assert_eq!(persisted.termination_reason, None);
    assert_eq!(fs::read_to_string(&state_path).unwrap(), original_state);
}
