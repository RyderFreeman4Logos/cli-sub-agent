use super::*;
use crate::cli::{Cli, Commands, SessionCommands};
use clap::Parser;

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
fn attach_primary_output_uses_stdout_for_runtime_binary_missing_without_output_log() {
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
        AttachPrimaryOutput::StdoutLog
    );
}

fn try_parse_cli(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(args)
}

#[test]
fn session_attach_accepts_prompt_flag_and_prompt_file() {
    let cli = try_parse_cli(&[
        "csa",
        "session",
        "attach",
        "--session",
        "01KTEST1234567890ABCDEFGHJK",
        "--prompt",
        "resume work",
    ])
    .expect("attach --prompt should parse");
    match cli.command {
        Commands::Session {
            cmd:
                SessionCommands::Attach {
                    prompt_flag,
                    prompt_file,
                    ..
                },
        } => {
            assert_eq!(prompt_flag.as_deref(), Some("resume work"));
            assert!(prompt_file.is_none());
        }
        _ => panic!("expected session attach command"),
    }

    let cli = try_parse_cli(&[
        "csa",
        "session",
        "attach",
        "--session",
        "01KTEST1234567890ABCDEFGHJK",
        "--prompt-file",
        "/tmp/prompt.md",
    ])
    .expect("attach --prompt-file should parse");
    match cli.command {
        Commands::Session {
            cmd:
                SessionCommands::Attach {
                    prompt_flag,
                    prompt_file,
                    ..
                },
        } => {
            assert!(prompt_flag.is_none());
            assert_eq!(
                prompt_file.as_deref(),
                Some(std::path::Path::new("/tmp/prompt.md"))
            );
        }
        _ => panic!("expected session attach command"),
    }
}

#[test]
fn session_attach_rejects_prompt_and_prompt_file_together() {
    let result = try_parse_cli(&[
        "csa",
        "session",
        "attach",
        "--session",
        "01KTEST1234567890ABCDEFGHJK",
        "--prompt",
        "resume work",
        "--prompt-file",
        "/tmp/prompt.md",
    ]);
    let err = match result {
        Ok(_) => panic!("attach --prompt and --prompt-file must conflict"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("--prompt-file"), "error: {err}");
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
fn attach_primary_output_prefers_existing_output_log_for_legacy_gemini_cli_sessions() {
    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "gemini-cli".to_string(),
        tool_locked: true,
        runtime_binary: None,
    };
    let metadata_toml = toml::to_string_pretty(&metadata).expect("metadata toml");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        metadata_toml,
    )
    .expect("write metadata");
    std::fs::write(td.path().join("output.log"), "gemini output\n").expect("write output log");
    std::fs::write(td.path().join("stdout.log"), "").expect("write stdout log");

    assert_eq!(
        attach_primary_output_for_session(td.path()),
        AttachPrimaryOutput::OutputLog
    );
}

#[test]
fn attach_primary_output_uses_stdout_for_legacy_gemini_cli_sessions_without_output_log() {
    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "gemini-cli".to_string(),
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
fn handle_session_attach_with_prompt_accepts_non_claude_sessions_via_soft_fork() {
    let td = tempfile::tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    for tool in ["codex", "gemini-cli"] {
        let session =
            csa_session::create_session(project, Some("attach-non-claude"), None, Some(tool))
                .expect("create session");
        let session_id = session.meta_session_id.clone();
        handle_session_attach_with_prompt(
            session_id.clone(),
            false,
            Some(project.to_string_lossy().into_owned()),
            Some("resume".to_string()),
            None,
            None,
        )
        .unwrap_or_else(|err| panic!("{tool} attach resume should use soft fork: {err:#}"));

        let session_dir = csa_session::get_session_dir(project, &session_id).expect("session dir");
        assert!(
            session_dir.join("input").join("attach-prompt.txt").exists(),
            "attach should persist the prompt before soft-fork spawn for {tool}"
        );
        assert!(
            session_dir.join("daemon.pid").exists(),
            "attach soft-fork spawn should write daemon.pid for {tool}"
        );
    }
}

#[test]
fn build_attach_resume_args_selects_resume_flag_by_tool_mode() {
    let project_root = std::path::Path::new("/tmp/project");
    let prompt_path = std::path::Path::new("/tmp/session/input/attach-prompt.txt");
    for (tool, use_native_resume, expected_flag, unexpected_flag) in [
        ("claude-code", true, "--session", "--fork-from"),
        ("codex", false, "--fork-from", "--session"),
        ("gemini-cli", false, "--fork-from", "--session"),
    ] {
        let args = build_attach_resume_args(
            "01KTEST1234567890ABCDEFGHJK",
            project_root,
            tool,
            prompt_path,
            use_native_resume,
        );
        assert!(args.iter().any(|arg| arg == expected_flag));
        assert!(!args.iter().any(|arg| arg == unexpected_flag));
    }
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

#[cfg(target_os = "linux")]
fn attach_test_daemon_pid_record(pid: u32) -> String {
    format!("{pid}\n")
}

#[cfg(target_os = "macos")]
fn attach_test_daemon_pid_record(pid: u32) -> String {
    format!("{pid} 0\n")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn handle_session_attach_waits_for_live_daemon_before_consuming_completion_packet() {
    use std::sync::mpsc;
    use std::time::Duration;

    let td = tempfile::tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
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
    std::fs::write(
        session_dir.join("daemon.pid"),
        attach_test_daemon_pid_record(child.id()),
    )
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
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
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

// #1118 part C ────────────────────────────────────────────────────────────────
//
// Non-daemon (inline) sessions have no `daemon.pid` file but record their tool
// PID in `locks/<tool>.lock`. `handle_session_kill` must fall back to that PID
// and signal the entire process group, matching the daemon path's semantics.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn handle_session_kill_uses_lock_file_for_inline_session() {
    let td = tempfile::tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = csa_session::create_session(project, Some("kill-inline"), None, Some("codex"))
        .expect("create session");
    let session_id = session.meta_session_id.clone();
    let session_dir = csa_session::get_session_dir(project, &session_id).expect("session dir");

    // Inline session: NO daemon.pid file; instead the tool PID lives in
    // `locks/<tool>.lock` (JSON record matching csa-process::extract_pid).
    assert!(
        !session_dir.join("daemon.pid").exists(),
        "inline-session test must not have a daemon.pid file"
    );

    let mut child = spawn_daemon_like_process(&session_id);
    let child_pid = child.id();

    let locks_dir = session_dir.join("locks");
    std::fs::create_dir_all(&locks_dir).expect("create locks dir");
    std::fs::write(
        locks_dir.join("codex.lock"),
        format!(r#"{{"pid": {child_pid}}}"#),
    )
    .expect("write lock file");

    let reaper = std::thread::spawn(move || child.wait().expect("wait child"));

    handle_session_kill(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
    )
    .expect("inline session kill should succeed");

    let status = reaper.join().expect("reaper join");
    assert!(
        !status.success(),
        "inline session process should be terminated by session kill"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn handle_session_kill_accepts_legacy_stderr_pid() {
    let td = tempfile::tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
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

// Property-based attach-routing coverage moved to
// `session_cmds_daemon_attach_proptest.rs`.
