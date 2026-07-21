use super::*;
use std::io::Read;

fn write_executable_script(path: &Path, contents: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o755)
        .open(path)
        .expect("create executable test script");
    file.write_all(contents.as_bytes())
        .expect("write executable test script");
    file.sync_all().expect("sync executable test script");
}

/// Write a wrapper that LOGS every received arg on its own line, then
/// skips daemon-child prefix args until `--` and evals the rest.
///
/// The per-arg `arg=<token>` log is what
/// `test_daemon_spawn_supports_multi_word_subcommand` inspects to prove
/// that the multi-word subcommand was actually split into distinct
/// argv tokens (`plan` and `run`), not passed as a single
/// `"plan run"` token. Without this, the wrapper's pre-`--` consume
/// loop would discard the evidence and the assertion would pass
/// vacuously.
fn write_wrapper_script(dir: &std::path::Path, name: &str) -> PathBuf {
    use std::io::Write;
    let script = dir.join(name);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o755)
        .open(&script)
        .expect("create wrapper script");
    f.write_all(
        b"#!/bin/sh\n\
          # Log every received arg first (one per line) so tests can\n\
          # assert on how the spawner split the subcommand path.\n\
          for tok in \"$@\"; do\n  echo \"arg=$tok\"\ndone\n\
          # Then skip all args until '--' and eval the rest.\n\
          while [ \"$#\" -gt 0 ]; do\n  case \"$1\" in --) shift; break;; *) shift;; esac\ndone\n\
          eval \"$@\"\n",
    )
    .expect("write wrapper script");
    f.sync_all().expect("sync wrapper script");
    drop(f);
    script
}

fn test_spawn_config(session_id: &str, csa_binary: PathBuf) -> DaemonSpawnConfig {
    DaemonSpawnConfig {
        session_id: session_id.to_string(),
        session_dir: PathBuf::from("/tmp/session-unused"),
        csa_binary,
        subcommand: "plan run".to_string(),
        args: vec!["--flag".to_string(), "value".to_string()],
        env: HashMap::from([("CSA_TEST_ENV".to_string(), "1".to_string())]),
    }
}

#[test]
fn test_build_daemon_command_direct_preserves_child_args() {
    let config = test_spawn_config("TESTDIRECT", PathBuf::from("/bin/csa-test"));
    let cmd = build_daemon_command(&config, &DaemonSpawnMode::Direct);

    let args: Vec<_> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    assert_eq!(cmd.get_program().to_string_lossy(), "/bin/csa-test");
    assert_eq!(
        args,
        vec![
            "plan",
            "run",
            "--daemon-child",
            "--session-id",
            "TESTDIRECT",
            "--flag",
            "value",
        ]
    );
}

#[test]
fn test_build_daemon_command_independent_scope_wraps_systemd_run() {
    let config = test_spawn_config("TESTSCOPE", PathBuf::from("/bin/csa-test"));
    let cmd = build_daemon_command(
        &config,
        &DaemonSpawnMode::IndependentScope {
            unit: "csa-daemon-TESTSCOPE.scope".to_string(),
        },
    );

    let args: Vec<_> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    assert_eq!(cmd.get_program().to_string_lossy(), "systemd-run");
    assert_eq!(
        args,
        vec![
            "--user",
            "--scope",
            "--quiet",
            "--collect",
            "--unit",
            "csa-daemon-TESTSCOPE.scope",
            "--",
            "/bin/csa-test",
            "plan",
            "run",
            "--daemon-child",
            "--session-id",
            "TESTSCOPE",
            "--flag",
            "value",
        ]
    );
}

#[test]
fn test_daemon_spawn_creates_spool_files() {
    let _guard = force_direct_daemon_spawn_for_test();
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("session-test");
    let wrapper = write_wrapper_script(tmp.path(), "wrapper1.sh");

    let config = DaemonSpawnConfig {
        session_id: "TEST001".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: wrapper,
        subcommand: "run".to_string(),
        // After the injected flags, pass '--' then the real command.
        args: vec!["--".to_string(), "echo hello".to_string()],
        env: HashMap::new(),
    };

    let result = spawn_daemon(config).expect("spawn_daemon");
    assert_eq!(result.session_id, "TEST001");
    assert!(result.pid > 0);

    // Give the child time to write and exit.
    std::thread::sleep(std::time::Duration::from_millis(500));

    let stdout_path = session_dir.join("stdout.log");
    let stderr_path = session_dir.join("stderr.log");
    assert!(stdout_path.exists(), "stdout.log must exist");
    assert!(stderr_path.exists(), "stderr.log must exist");

    let mut contents = String::new();
    File::open(&stdout_path)
        .expect("open stdout.log")
        .read_to_string(&mut contents)
        .expect("read stdout.log");
    assert!(
        contents.contains("hello"),
        "stdout.log should contain 'hello', got: {contents:?}"
    );
}

#[test]
fn test_daemon_spawn_supports_multi_word_subcommand() {
    let _guard = force_direct_daemon_spawn_for_test();
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("session-multi");
    let wrapper = write_wrapper_script(tmp.path(), "wrapper-multi.sh");

    let config = DaemonSpawnConfig {
        session_id: "TEST_MULTI".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: wrapper,
        subcommand: "plan run".to_string(),
        // Keep the wrapper alive through spawn_daemon's initial liveness
        // inspection. The argument log is emitted before this command runs.
        args: vec![
            "--".to_string(),
            "echo got=$1,$2,$3,$4,$5; sleep 2".to_string(),
        ],
        env: HashMap::new(),
    };

    let result = spawn_daemon(config).expect("spawn_daemon");
    assert!(result.pid > 0);
    std::thread::sleep(std::time::Duration::from_millis(500));

    let mut contents = String::new();
    File::open(session_dir.join("stdout.log"))
        .expect("open stdout.log")
        .read_to_string(&mut contents)
        .expect("read stdout.log");

    // The wrapper logs every received arg as `arg=<token>` on its own
    // line BEFORE consuming up to `--`. The actual exec was:
    //   <wrapper> plan run --daemon-child --session-id TEST_MULTI -- echo got=...
    // Assert on the split: `plan` and `run` MUST appear on DISTINCT
    // arg= lines. Without distinct lines, `subcommand: "plan run"`
    // could have been passed as a single argv token and we wouldn't
    // notice — that was the original test's vacuous-pass bug
    // (#1130 PR-1 review F2).
    let arg_lines: Vec<&str> = contents.lines().filter(|l| l.starts_with("arg=")).collect();
    assert!(
        arg_lines.contains(&"arg=plan"),
        "expected a distinct `arg=plan` line proving the subcommand was \
         split, got arg lines: {arg_lines:?}"
    );
    assert!(
        arg_lines.contains(&"arg=run"),
        "expected a distinct `arg=run` line proving the subcommand was \
         split, got arg lines: {arg_lines:?}"
    );
    // Sanity: the daemon-child prefix the spawner injects must also be
    // present so we know we're inspecting the real exec, not a noop.
    assert!(
        arg_lines.contains(&"arg=--daemon-child"),
        "expected `arg=--daemon-child` from the spawner injection, got \
         arg lines: {arg_lines:?}"
    );
    assert!(
        contents.contains("got="),
        "stdout should still contain 'got=' (exec ran), got: {contents:?}"
    );
}

/// Verify that when IndependentScope is forced but systemd-run cannot be
/// spawned, spawn_daemon retries with Direct mode and still succeeds.
///
/// Simulates the nested-CSA-subprocess scenario where the env override forces
/// IndependentScope but dbus / systemd-run is absent (ENOENT at exec time).
#[test]
fn test_daemon_spawn_falls_back_to_direct_when_scope_spawn_fails() {
    let _env_guard = force_independent_scope_for_test();

    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("session-fallback");
    let wrapper = write_wrapper_script(tmp.path(), "wrapper-fallback.sh");

    let missing_systemd_run = tmp.path().join("missing-systemd-run");

    let config = DaemonSpawnConfig {
        session_id: "TEST_FALLBACK".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: wrapper,
        subcommand: "run".to_string(),
        args: vec!["--".to_string(), "echo scope-fallback-ok".to_string()],
        env: HashMap::new(),
    };

    let result = spawn_daemon_with_systemd_run(config, &missing_systemd_run);

    let result = result.expect("spawn_daemon should succeed via direct fallback");
    assert!(result.pid > 0);

    std::thread::sleep(std::time::Duration::from_millis(500));

    let contents = std::fs::read_to_string(session_dir.join("stdout.log"))
        .expect("stdout.log must exist after daemon spawn");
    assert!(
        contents.contains("scope-fallback-ok"),
        "expected 'scope-fallback-ok' in stdout (direct fallback output), got: {contents:?}"
    );
}

#[test]
fn test_daemon_spawn_child_detached() {
    let _guard = force_direct_daemon_spawn_for_test();
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("session-detach");
    let wrapper = write_wrapper_script(tmp.path(), "wrapper2.sh");

    let config = DaemonSpawnConfig {
        session_id: "TEST002".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: wrapper,
        subcommand: "run".to_string(),
        args: vec![
            "--".to_string(),
            "echo pid=$$ sid=$(ps -o sid= -p $$)".to_string(),
        ],
        env: HashMap::new(),
    };

    let result = spawn_daemon(config).expect("spawn_daemon");
    let child_pid = result.pid;
    let parent_pid = std::process::id();

    assert_ne!(child_pid, parent_pid, "child PID must differ from parent");

    // Give the child time to write and exit.
    std::thread::sleep(std::time::Duration::from_millis(500));

    let mut contents = String::new();
    File::open(session_dir.join("stdout.log"))
        .expect("open stdout.log")
        .read_to_string(&mut contents)
        .expect("read stdout.log");

    // Parse the sid= value from output and verify it differs from
    // the parent's session ID.
    if let Some(sid_str) = contents.split("sid=").nth(1) {
        let child_sid: u32 = sid_str.trim().parse().unwrap_or(0);
        // SAFETY: libc::getsid is safe for the current process.
        let parent_sid = unsafe { libc::getsid(0) } as u32;
        assert_ne!(
            child_sid, parent_sid,
            "child session ID ({child_sid}) must differ from parent ({parent_sid})"
        );
    }
}

#[test]
fn verified_scope_failure_stops_the_recorded_systemd_unit() {
    let _guard = force_independent_scope_for_test();
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("session-scope-cleanup");
    let wrapper = write_wrapper_script(tmp.path(), "wrapper-scope-cleanup.sh");
    let fake_systemd_run = tmp.path().join("systemd-run");
    write_executable_script(
        &fake_systemd_run,
        "#!/bin/sh\nwhile [ \"$#\" -gt 0 ]; do\n  case \"$1\" in\n    --) shift; exec \"$@\";;\n    *) shift;;\n  esac\ndone\nexit 64\n",
    );
    let systemctl_log = tmp.path().join("systemctl.log");
    let fake_systemctl = tmp.path().join("systemctl");
    write_executable_script(
        &fake_systemctl,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
            systemctl_log.display()
        ),
    );
    let config = DaemonSpawnConfig {
        session_id: "TEST_SCOPE_CLEANUP".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: wrapper,
        subcommand: "run".to_string(),
        args: vec![
            "--".to_string(),
            "trap '' TERM; while :; do sleep 1; done".to_string(),
        ],
        env: HashMap::new(),
    };

    let err =
        spawn_daemon_verified_with_commands(config, &fake_systemd_run, &fake_systemctl, |_| {
            anyhow::bail!("scope readiness failed")
        })
        .err()
        .expect("verification failure must fail the spawn");

    assert!(format!("{err:#}").contains("scope readiness failed"));
    assert_eq!(
        std::fs::read_to_string(systemctl_log).expect("systemctl invocation should be logged"),
        "--user\nstop\ncsa-daemon-TEST_SCOPE_CLEANUP.scope\n"
    );
    assert!(!session_dir.join("daemon.pid").exists());
    assert!(!session_dir.join("daemon.scope").exists());
}

#[test]
fn verified_scope_failure_preserves_cleanup_failure_context() {
    let _guard = force_independent_scope_for_test();
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("session-scope-cleanup-error");
    let wrapper = write_wrapper_script(tmp.path(), "wrapper-scope-cleanup-error.sh");
    let fake_systemd_run = tmp.path().join("systemd-run");
    write_executable_script(
        &fake_systemd_run,
        "#!/bin/sh\nwhile [ \"$#\" -gt 0 ]; do\n  case \"$1\" in\n    --) shift; exec \"$@\";;\n    *) shift;;\n  esac\ndone\nexit 64\n",
    );
    let fake_systemctl = tmp.path().join("systemctl");
    write_executable_script(&fake_systemctl, "#!/bin/sh\nexit 17\n");
    let config = DaemonSpawnConfig {
        session_id: "TEST_SCOPE_CLEANUP_ERROR".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: wrapper,
        subcommand: "run".to_string(),
        args: vec![
            "--".to_string(),
            "trap '' TERM; while :; do sleep 1; done".to_string(),
        ],
        env: HashMap::new(),
    };

    let err =
        spawn_daemon_verified_with_commands(config, &fake_systemd_run, &fake_systemctl, |_| {
            anyhow::bail!("scope readiness failed")
        })
        .err()
        .expect("verification and cleanup failures must be returned");
    let rendered = format!("{err:#}");

    assert!(rendered.contains("scope readiness failed"), "{rendered}");
    assert!(
        rendered.contains("systemctl") && rendered.contains("status 17"),
        "cleanup context missing from error: {rendered}"
    );
    assert!(
        session_dir.join("daemon.pid").exists() && session_dir.join("daemon.scope").exists(),
        "failed cleanup must retain exact lifecycle records for diagnosis/recovery"
    );
}

#[test]
fn verified_spawn_rejects_a_direct_daemon_that_exited_during_verification() {
    let _guard = force_direct_daemon_spawn_for_test();
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("session-verification-exit");
    let wrapper = write_wrapper_script(tmp.path(), "wrapper-verification-exit.sh");
    let config = DaemonSpawnConfig {
        session_id: "TEST_VERIFY_EXIT".to_string(),
        session_dir,
        csa_binary: wrapper,
        subcommand: "run".to_string(),
        args: vec!["--".to_string(), "exit 0".to_string()],
        env: HashMap::new(),
    };

    let err = spawn_daemon_verified(config, |_| {
        std::thread::sleep(std::time::Duration::from_millis(100));
        Ok(())
    })
    .err()
    .expect("an exited daemon must not be reported as successfully detached");

    assert!(
        format!("{err:#}").contains("exited before readiness verification completed"),
        "unexpected error: {err:#}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn verified_spawn_failure_terminates_and_reaps_before_marker_emission() {
    let _guard = force_direct_daemon_spawn_for_test();
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("session-verification-failure");
    let wrapper = write_wrapper_script(tmp.path(), "wrapper-verification-failure.sh");
    let config = DaemonSpawnConfig {
        session_id: "TEST_VERIFY_FAIL".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: wrapper,
        subcommand: "plan run".to_string(),
        args: vec![
            "--".to_string(),
            "trap '' TERM; while :; do sleep 1; done".to_string(),
        ],
        env: HashMap::new(),
    };

    let mut spawned_pid = None;
    let err = spawn_daemon_verified(config, |result| {
        spawned_pid = Some(result.pid as libc::pid_t);
        anyhow::bail!("lookup verification failed")
    })
    .err()
    .expect("verification failure must prevent a successful detached spawn");
    assert!(format!("{err:#}").contains("lookup verification failed"));

    let pid = spawned_pid.expect("verification callback should observe spawned PID");
    assert!(
        !session_dir.join("daemon.pid").exists(),
        "successful cleanup must remove the stale daemon PID record"
    );
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "verification failure must terminate the spawned daemon process"
    );

    let mut status = 0;
    // SAFETY: `pid` came from the child spawned by this process. WNOHANG
    // only queries whether that exact child still has an unreaped status.
    let wait_result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    assert_eq!(wait_result, -1, "cleanup must already reap the child");

    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD),
        "the spawned child must no longer be waitable by its parent"
    );
}
