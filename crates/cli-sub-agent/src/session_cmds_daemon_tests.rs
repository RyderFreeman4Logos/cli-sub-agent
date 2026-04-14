use super::*;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::test_env_lock::TEST_ENV_LOCK;

#[cfg(any(target_os = "linux", target_os = "macos"))]
struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
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

#[cfg(unix)]
fn set_file_mtime_seconds_ago(path: &std::path::Path, seconds_ago: u64) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch");
    let target = now.saturating_sub(std::time::Duration::from_secs(seconds_ago));
    let tv_sec = target.as_secs() as libc::time_t;
    let tv_nsec = target.subsec_nanos() as libc::c_long;
    let times = [
        libc::timespec { tv_sec, tv_nsec },
        libc::timespec { tv_sec, tv_nsec },
    ];
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("path contains NUL");
    // SAFETY: `utimensat` uses a valid path and stack-allocated timespec array.
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
    assert_eq!(rc, 0, "utimensat failed for {}", path.display());
}

#[test]
fn attach_primary_output_prefers_output_log_for_acp_tools() {
    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "claude-code".to_string(),
        tool_locked: true,
        runtime_binary: None,
    };
    let metadata_toml = toml::to_string_pretty(&metadata).expect("metadata toml");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        metadata_toml,
    )
    .expect("write metadata");

    assert_eq!(
        attach_primary_output_for_session(td.path()),
        AttachPrimaryOutput::OutputLog
    );
}

#[test]
fn attach_primary_output_prefers_existing_output_log_for_codex_sessions() {
    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "codex".to_string(),
        tool_locked: true,
        runtime_binary: None,
    };
    let metadata_toml = toml::to_string_pretty(&metadata).expect("metadata toml");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        metadata_toml,
    )
    .expect("write metadata");
    std::fs::write(td.path().join("output.log"), "").expect("write output log");

    assert_eq!(
        attach_primary_output_for_session(td.path()),
        AttachPrimaryOutput::OutputLog
    );
}

#[test]
fn attach_primary_output_preserves_legacy_codex_output_log_when_runtime_binary_missing() {
    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "codex".to_string(),
        tool_locked: true,
        runtime_binary: None,
    };
    let metadata_toml = toml::to_string_pretty(&metadata).expect("metadata toml");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        metadata_toml,
    )
    .expect("write metadata");
    std::fs::write(td.path().join("output.log"), "").expect("write output log");
    std::fs::write(td.path().join("stdout.log"), "").expect("write stdout log");

    assert_eq!(
        attach_primary_output_for_session(td.path()),
        AttachPrimaryOutput::OutputLog
    );
}

#[test]
fn attach_primary_output_preserves_output_log_for_unresolved_live_codex_session() {
    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "codex".to_string(),
        tool_locked: true,
        runtime_binary: None,
    };
    let metadata_toml = toml::to_string_pretty(&metadata).expect("metadata toml");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        metadata_toml,
    )
    .expect("write metadata");
    std::fs::write(td.path().join("stdout.log"), "").expect("write stdout log");
    std::fs::create_dir_all(td.path().join("locks")).expect("create locks dir");
    std::fs::write(
        td.path().join("locks").join("codex.lock"),
        format!("{{\"pid\":{}}}", std::process::id()),
    )
    .expect("write codex lock");

    assert_eq!(
        attach_primary_output_for_session(td.path()),
        AttachPrimaryOutput::OutputLog
    );
}

#[test]
fn attach_primary_output_keeps_stdout_for_non_codex_sessions_with_output_log() {
    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "opencode".to_string(),
        tool_locked: true,
        runtime_binary: None,
    };
    let metadata_toml = toml::to_string_pretty(&metadata).expect("metadata toml");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        metadata_toml,
    )
    .expect("write metadata");
    std::fs::write(td.path().join("output.log"), "").expect("write output log");
    std::fs::write(td.path().join("stdout.log"), "").expect("write stdout log");

    assert_eq!(
        attach_primary_output_for_session(td.path()),
        AttachPrimaryOutput::StdoutLog
    );
}

#[test]
fn attach_primary_output_keeps_stdout_for_legacy_tools() {
    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "opencode".to_string(),
        tool_locked: true,
        runtime_binary: None,
    };
    let metadata_toml = toml::to_string_pretty(&metadata).expect("metadata toml");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        metadata_toml,
    )
    .expect("write metadata");

    assert_eq!(
        attach_primary_output_for_session(td.path()),
        AttachPrimaryOutput::StdoutLog
    );
}

#[test]
fn attach_primary_output_uses_persisted_codex_acp_runtime_binary() {
    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "codex".to_string(),
        tool_locked: true,
        runtime_binary: Some("codex-acp".to_string()),
    };
    let metadata_toml = toml::to_string_pretty(&metadata).expect("metadata toml");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        metadata_toml,
    )
    .expect("write metadata");

    assert_eq!(
        attach_primary_output_for_session(td.path()),
        AttachPrimaryOutput::OutputLog
    );
}

#[test]
fn attach_primary_output_uses_output_log_when_codex_acp_hint_survives_invalid_metadata() {
    let td = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        "tool = \"codex\"\nruntime_binary = \"codex-acp\"\ntool_locked = \n",
    )
    .expect("write invalid metadata");

    assert_eq!(
        attach_primary_output_for_session(td.path()),
        AttachPrimaryOutput::OutputLog
    );
}

#[test]
fn wait_for_attach_live_output_path_keeps_waiting_past_sixty_seconds_for_codex_output_log() {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use std::time::Duration;

    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "codex".to_string(),
        tool_locked: true,
        runtime_binary: Some("codex-acp".to_string()),
    };
    let metadata_toml = toml::to_string_pretty(&metadata).expect("metadata toml");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        metadata_toml,
    )
    .expect("write metadata");
    std::fs::create_dir_all(td.path().join("locks")).expect("create locks dir");
    std::fs::write(
        td.path().join("locks").join("codex.lock"),
        format!("{{\"pid\":{}}}", std::process::id()),
    )
    .expect("write codex lock");

    let stdout_path = td.path().join("stdout.log");
    let output_path = td.path().join("output.log");
    let elapsed_ms = Arc::new(AtomicU64::new(0));
    let sleep_elapsed_ms = Arc::clone(&elapsed_ms);
    let delayed_output_path = output_path.clone();

    let resolved = wait_for_attach_live_output_path(
        td.path(),
        "attach-60s-codex-output",
        &stdout_path,
        &output_path,
        || Duration::from_millis(elapsed_ms.load(Ordering::Relaxed)),
        move |duration| {
            let elapsed = sleep_elapsed_ms
                .fetch_add(duration.as_millis() as u64, Ordering::Relaxed)
                + duration.as_millis() as u64;
            if elapsed >= 61_000 && !delayed_output_path.exists() {
                std::fs::write(&delayed_output_path, "late acp output\n")
                    .expect("write delayed output log");
            }
        },
    )
    .expect("wait should not fail");

    assert_eq!(resolved, Some(output_path));
    assert!(
        elapsed_ms.load(Ordering::Relaxed) >= 61_000,
        "attach should keep waiting well past the old 30s failure threshold"
    );
}

#[test]
fn wait_for_attach_live_output_path_uses_stdout_for_live_stdout_when_metadata_is_unreadable() {
    use std::time::Duration;

    let td = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        "tool = \n",
    )
    .expect("write unreadable metadata");
    std::fs::write(td.path().join("stdout.log"), "live stdout\n").expect("write stdout log");
    std::fs::create_dir_all(td.path().join("locks")).expect("create locks dir");
    std::fs::write(
        td.path().join("locks").join("codex.lock"),
        format!("{{\"pid\":{}}}", std::process::id()),
    )
    .expect("write codex lock");

    let stdout_path = td.path().join("stdout.log");
    let output_path = td.path().join("output.log");

    let resolved = wait_for_attach_live_output_path(
        td.path(),
        "attach-live-stdout-unreadable-metadata",
        &stdout_path,
        &output_path,
        || Duration::ZERO,
        |_| panic!("attach should not sleep when stdout is already live"),
    )
    .expect("wait should not fail");

    assert_eq!(resolved, Some(stdout_path));
}

#[cfg(unix)]
#[test]
fn wait_for_attach_live_output_path_falls_back_to_stdout_after_metadata_grace_window() {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use std::time::Duration;

    let td = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        "tool = \n",
    )
    .expect("write unreadable metadata");
    std::fs::write(td.path().join("stdout.log"), "live stdout\n").expect("write stdout log");
    set_file_mtime_seconds_ago(&td.path().join("stdout.log"), 60);
    std::fs::create_dir_all(td.path().join("locks")).expect("create locks dir");
    std::fs::write(
        td.path().join("locks").join("codex.lock"),
        format!("{{\"pid\":{}}}", std::process::id()),
    )
    .expect("write codex lock");

    let stdout_path = td.path().join("stdout.log");
    let output_path = td.path().join("output.log");
    let elapsed_ms = Arc::new(AtomicU64::new(0));
    let sleep_elapsed_ms = Arc::clone(&elapsed_ms);

    let resolved = wait_for_attach_live_output_path(
        td.path(),
        "attach-unreadable-metadata-stdout-grace",
        &stdout_path,
        &output_path,
        || Duration::from_millis(elapsed_ms.load(Ordering::Relaxed)),
        move |duration| {
            sleep_elapsed_ms.fetch_add(duration.as_millis() as u64, Ordering::Relaxed);
        },
    )
    .expect("wait should not fail");

    assert_eq!(resolved, Some(stdout_path));
    assert!(
        elapsed_ms.load(Ordering::Relaxed)
            >= ATTACH_METADATA_STDOUT_GRACE_WINDOW.as_millis() as u64,
        "attach should fall back to stdout after a short unresolved-metadata grace window"
    );
}

#[test]
fn attach_primary_output_uses_stdout_for_persisted_codex_cli_runtime_binary() {
    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "codex".to_string(),
        tool_locked: true,
        runtime_binary: Some("codex".to_string()),
    };
    let metadata_toml = toml::to_string_pretty(&metadata).expect("metadata toml");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        metadata_toml,
    )
    .expect("write metadata");
    std::fs::write(td.path().join("stdout.log"), "").expect("write stdout log");

    assert_eq!(
        attach_primary_output_for_session(td.path()),
        AttachPrimaryOutput::StdoutLog
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn spawn_daemon_like_process(session_id: &str) -> std::process::Child {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    let mut cmd = Command::new("sh");
    cmd.args(["-c", "sleep 60", "csa-daemon", session_id]);
    // SAFETY: test fixture only; makes the child its own session leader like a daemon.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn().expect("spawn daemon-like child")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn handle_session_attach_waits_for_live_daemon_before_consuming_completion_packet() {
    use std::sync::mpsc;
    use std::time::Duration;

    let td = tempfile::tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = csa_session::create_session(
        project,
        Some("attach-completion-gate"),
        None,
        Some("opencode"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = csa_session::get_session_dir(project, &session_id).expect("session dir");
    std::fs::write(session_dir.join("stdout.log"), "").expect("write stdout log");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 1\nstatus = \"failure\"\n",
    )
    .expect("write completion packet");

    let mut child = spawn_daemon_like_process(&session_id);
    std::fs::write(session_dir.join("daemon.pid"), format!("{}\n", child.id()))
        .expect("write daemon pid");
    let daemon_visible = (0..20).any(|_| {
        if csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir) {
            true
        } else {
            std::thread::sleep(Duration::from_millis(25));
            false
        }
    });
    assert!(
        daemon_visible,
        "fixture must observe daemon.pid liveness before attach starts"
    );

    let (tx, rx) = mpsc::channel();
    let attach_session = session_id.clone();
    let attach_project = project.to_string_lossy().into_owned();
    let handle = std::thread::spawn(move || {
        let result = handle_session_attach(attach_session, false, Some(attach_project))
            .map_err(|err| err.to_string());
        let _ = tx.send(result);
    });

    assert!(
        matches!(
            rx.recv_timeout(Duration::from_millis(500)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "attach must keep tailing while the daemon process is still alive"
    );

    child.kill().ok();
    child.wait().ok();

    let exit_code = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("attach should finish after the daemon exits")
        .expect("attach result");
    handle.join().expect("attach thread join");
    assert_eq!(exit_code, 1);
}

#[cfg(target_os = "linux")]
#[test]
fn handle_session_attach_treats_stale_daemon_pid_as_dead() {
    use std::sync::mpsc;
    use std::time::Duration;

    let td = tempfile::tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = csa_session::create_session(
        project,
        Some("attach-stale-daemon-pid"),
        None,
        Some("opencode"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = csa_session::get_session_dir(project, &session_id).expect("session dir");
    std::fs::write(session_dir.join("stdout.log"), "").expect("write stdout log");

    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("spawn child");
    std::fs::write(
        session_dir.join("daemon.pid"),
        format!("{} 0\n", child.id()),
    )
    .expect("write daemon pid");

    let (tx, rx) = mpsc::channel();
    let attach_session = session_id.clone();
    let attach_project = project.to_string_lossy().into_owned();
    let handle = std::thread::spawn(move || {
        let result = handle_session_attach(attach_session, false, Some(attach_project))
            .map_err(|err| err.to_string());
        let _ = tx.send(result);
    });

    let attach_result = rx.recv_timeout(Duration::from_secs(2));

    child.kill().ok();
    child.wait().ok();
    handle.join().expect("attach thread join");

    let exit_code = attach_result
        .expect("attach should converge instead of waiting on a reused PID")
        .expect("attach result");
    assert_eq!(exit_code, 1);
    let result = csa_session::load_result(project, &session_id)
        .expect("load result")
        .expect("synthetic result");
    assert_eq!(result.status, "failure");
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn handle_session_kill_accepts_legacy_stderr_pid() {
    let td = tempfile::tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session =
        csa_session::create_session(project, Some("kill-legacy-stderr"), None, Some("opencode"))
            .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = csa_session::get_session_dir(project, &session_id).expect("session dir");

    let mut child = spawn_daemon_like_process(&session_id);
    let child_pid = child.id();
    std::fs::write(
        session_dir.join("stderr.log"),
        format!(
            "<!-- CSA:SESSION_STARTED id={} pid={} dir=\"{}\" wait_cmd=\"\" attach_cmd=\"\" -->\n",
            session_id,
            child_pid,
            session_dir.display()
        ),
    )
    .expect("write legacy stderr pid");

    let reaper = std::thread::spawn(move || child.wait().expect("wait child"));

    handle_session_kill(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
    )
    .expect("legacy kill should succeed");

    let status = reaper.join().expect("reaper join");
    assert!(
        !status.success(),
        "legacy daemon process should be terminated by session kill"
    );
}
