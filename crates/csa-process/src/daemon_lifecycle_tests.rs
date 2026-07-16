use super::*;

#[cfg(target_os = "linux")]
use std::cell::RefCell;
use std::ffi::CString;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::rc::Rc;
use std::time::{Duration, Instant};

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

fn write_wrapper_script(dir: &Path, name: &str) -> PathBuf {
    let script = dir.join(name);
    write_executable_script(
        &script,
        "#!/bin/sh\nwhile [ \"$#\" -gt 0 ]; do\n  case \"$1\" in\n    --) shift; break;;\n    *) shift;;\n  esac\ndone\neval \"$@\"\n",
    );
    script
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    predicate()
}

#[cfg(target_os = "linux")]
struct FixtureFifo(std::fs::File);

#[cfg(target_os = "linux")]
impl FixtureFifo {
    fn create(path: &Path) -> Self {
        let path_c = CString::new(path.as_os_str().as_bytes()).expect("FIFO path contains NUL");
        // SAFETY: path_c is a valid NUL-terminated path and mode contains only
        // permission bits. The temporary directory owns the resulting FIFO.
        let rc = unsafe { libc::mkfifo(path_c.as_ptr(), 0o600) };
        assert_eq!(
            rc,
            0,
            "create fixture FIFO {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .expect("open fixture FIFO");
        Self(file)
    }

    fn read_markers(&mut self, count: usize, timeout: Duration) -> anyhow::Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut markers = Vec::with_capacity(count);
        while markers.len() < count {
            let remaining = deadline.saturating_duration_since(Instant::now());
            anyhow::ensure!(!remaining.is_zero(), "fixture FIFO handshake timed out");
            let timeout_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
            let mut pollfd = libc::pollfd {
                fd: std::os::fd::AsRawFd::as_raw_fd(&self.0),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: pollfd points to one initialized descriptor for the
            // duration of this bounded poll call.
            let poll_rc = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
            if poll_rc == -1 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(anyhow::anyhow!("fixture FIFO poll failed: {error}"));
            }
            anyhow::ensure!(poll_rc != 0, "fixture FIFO handshake timed out");
            anyhow::ensure!(
                pollfd.revents & libc::POLLIN != 0,
                "fixture FIFO returned unexpected poll events: {}",
                pollfd.revents,
            );

            let mut buffer = [0_u8; 3];
            let remaining_count = count - markers.len();
            let read_length = remaining_count.min(buffer.len());
            match self.0.read(&mut buffer[..read_length]) {
                Ok(0) => anyhow::bail!("fixture FIFO closed before handshake completed"),
                Ok(read) => markers.extend_from_slice(&buffer[..read]),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                    ) => {}
                Err(error) => return Err(anyhow::anyhow!("fixture FIFO read failed: {error}")),
            }
        }
        Ok(markers)
    }
}

#[cfg(target_os = "linux")]
fn wait_for_unreaped_child_exit(pid: u32, timeout: Duration) -> anyhow::Result<()> {
    anyhow::ensure!(pid > 1, "invalid fixture leader PID {pid}");
    let deadline = Instant::now() + timeout;
    loop {
        // SAFETY: zero is the portable no-state-change sentinel for waitid
        // with WNOHANG, and info remains valid for the duration of the call.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: P_PID selects the owned fixture child. WNOWAIT proves exit
        // without reaping it, preserving its PID as the process-group anchor.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if rc == -1 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(anyhow::anyhow!(
                "waitid(WNOWAIT) failed for fixture leader {pid}: {error}"
            ));
        }
        // SAFETY: waitid initialized info; si_pid == 0 means no state change.
        let exited_pid = unsafe { info.si_pid() };
        if exited_pid != 0 {
            anyhow::ensure!(
                exited_pid == pid as libc::pid_t,
                "waitid returned unexpected fixture PID {exited_pid}, expected {pid}"
            );
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "fixture leader {pid} did not exit before the bounded deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct ProcessGuard(Option<libc::pid_t>);

impl ProcessGuard {
    fn new(pid: libc::pid_t) -> Self {
        Self(Some(pid))
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        // SAFETY: the test records this PID from its own fixture and uses the
        // guard only as best-effort cleanup if the assertion fails.
        if let Some(pid) = self.0 {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

fn process_exists(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only probes whether the recorded process exists.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(target_os = "linux")]
fn process_is_live(pid: libc::pid_t) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(after_comm) = stat.rsplit_once(')').map(|(_, rest)| rest.trim_start()) else {
        return false;
    };
    !matches!(after_comm.as_bytes().first(), Some(b'Z' | b'X'))
}

#[cfg(not(target_os = "linux"))]
fn process_is_live(pid: libc::pid_t) -> bool {
    process_exists(pid)
}

#[test]
fn systemd_scope_stop_times_out_and_reaps_the_systemctl_process() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let systemctl = tmp.path().join("systemctl");
    let systemctl_pid_file = tmp.path().join("systemctl.pid");
    write_executable_script(
        &systemctl,
        &format!(
            "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\ntrap '' TERM\nwhile :; do sleep 1; done\n",
            systemctl_pid_file.display()
        ),
    );

    let started = Instant::now();
    let err = stop_systemd_scope_with_timeout(
        &systemctl,
        "csa-daemon-timeout.scope",
        Duration::from_millis(100),
    )
    .expect_err("a hung systemctl must time out");

    assert!(
        started.elapsed() < Duration::from_secs(1),
        "scope cleanup exceeded its bounded deadline: {:?}",
        started.elapsed()
    );
    assert!(format!("{err:#}").contains("timed out"), "{err:#}");
    let systemctl_pid = std::fs::read_to_string(systemctl_pid_file)
        .expect("systemctl fixture should record its PID")
        .parse::<libc::pid_t>()
        .expect("systemctl PID should parse");
    let mut process_guard = ProcessGuard::new(systemctl_pid);
    assert!(
        !process_exists(systemctl_pid),
        "timed-out systemctl PID {systemctl_pid} must already be reaped"
    );
    process_guard.disarm();
}

#[test]
fn verified_failure_removes_lifecycle_records_but_keeps_diagnostic_logs() {
    let _guard = force_direct_daemon_spawn_for_test();
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("session-verification-failure");
    let config = DaemonSpawnConfig {
        session_id: "TEST_VERIFY_FAIL_ARTIFACTS".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: write_wrapper_script(tmp.path(), "wrapper-artifacts.sh"),
        subcommand: "run".to_string(),
        args: vec![
            "--".to_string(),
            "trap '' TERM; while :; do sleep 1; done".to_string(),
        ],
        env: HashMap::new(),
    };

    spawn_daemon_verified(config, |_| anyhow::bail!("lookup verification failed"))
        .err()
        .expect("verification failure must fail the spawn");

    assert!(
        !session_dir.join("daemon.pid").exists(),
        "a cleaned failed launch must not retain a misleading daemon.pid"
    );
    assert!(
        !session_dir.join("daemon.scope").exists(),
        "direct cleanup must not retain daemon.scope"
    );
    assert!(
        session_dir.join("stdout.log").exists() && session_dir.join("stderr.log").exists(),
        "spool logs must remain available as launch diagnostics"
    );
}

#[test]
fn unknown_direct_wait_state_preserves_lifecycle_records() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("session-unknown-wait-state");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    let result = DaemonSpawnResult {
        pid: 42_424,
        session_id: "TEST_UNKNOWN_WAIT_STATE".to_string(),
        session_dir: session_dir.clone(),
    };
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(result.pid),
    )
    .expect("pid record");

    let error = cleanup_after_spawn_error(
        anyhow::anyhow!("wait probe failed"),
        "spawned-daemon liveness check failed",
        SpawnedProcessCleanup::WaitStateUnknown,
        &DaemonSpawnMode::Direct,
        Path::new("unused-systemctl"),
        &result,
    );

    assert!(
        format!("{error:#}").contains("cannot safely signal or reap"),
        "{error:#}"
    );
    assert!(
        session_dir.join("daemon.pid").is_file(),
        "an unmanaged daemon must retain its lifecycle record for recovery"
    );
}

#[test]
fn publication_failure_cleans_the_still_owned_daemon() {
    let _guard = force_direct_daemon_spawn_for_test();
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("session-publication-failure");
    let config = DaemonSpawnConfig {
        session_id: "TEST_PUBLISH_FAIL".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: write_wrapper_script(tmp.path(), "wrapper-publish.sh"),
        subcommand: "run".to_string(),
        args: vec![
            "--".to_string(),
            "trap '' TERM; while :; do sleep 1; done".to_string(),
        ],
        env: HashMap::new(),
    };

    let err = spawn_daemon_verified_and_publish(
        config,
        |_| Ok("ready"),
        |_, _| anyhow::bail!("marker write failed"),
    )
    .err()
    .expect("publication failure must fail the spawn");

    assert!(format!("{err:#}").contains("marker write failed"));
    assert!(!session_dir.join("daemon.pid").exists());
    assert!(session_dir.join("stderr.log").exists());
}

#[test]
fn early_exited_direct_leader_remains_anchor_until_descendants_are_killed() {
    let _guard = force_direct_daemon_spawn_for_test();
    let tmp = tempfile::tempdir().expect("tempdir");
    let descendant_pid_file = tmp.path().join("descendant.pid");
    let readiness_fifo_path = tmp.path().join("descendant-ready.fifo");
    let mut readiness_fifo = FixtureFifo::create(&readiness_fifo_path);
    let session_dir = tmp.path().join("session-descendant-cleanup");
    let command = format!(
        "sh -c 'trap \"\" TERM; printf \"%s\" \"$$\" > \"{}\"; printf D > \"{}\"; \
         while :; do sleep 1; done' & exit 0",
        descendant_pid_file.display(),
        readiness_fifo_path.display()
    );
    let config = DaemonSpawnConfig {
        session_id: "TEST_EARLY_EXIT_DESCENDANT".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: write_wrapper_script(tmp.path(), "wrapper-descendant.sh"),
        subcommand: "run".to_string(),
        args: vec!["--".to_string(), command],
        env: HashMap::new(),
    };

    let err = spawn_daemon_verified(config, |result| {
        anyhow::ensure!(
            readiness_fifo.read_markers(1, Duration::from_secs(1))? == b"D",
            "descendant fixture sent an invalid readiness marker"
        );
        wait_for_unreaped_child_exit(result.pid, Duration::from_secs(1))?;
        Ok(())
    })
    .err()
    .expect("the exited daemon leader must be rejected");
    assert!(
        format!("{err:#}").contains("exited before readiness"),
        "{err:#}"
    );

    let descendant_pid = std::fs::read_to_string(&descendant_pid_file)
        .expect("read descendant pid")
        .trim()
        .parse::<libc::pid_t>()
        .expect("parse descendant pid");
    let mut process_guard = ProcessGuard::new(descendant_pid);
    assert!(
        wait_until(Duration::from_secs(1), || !process_is_live(descendant_pid)),
        "anchored cleanup must kill descendant {descendant_pid} after the leader exits"
    );
    process_guard.disarm();
    assert!(!session_dir.join("daemon.pid").exists());
    assert!(!session_dir.join("daemon.scope").exists());
    assert!(session_dir.join("stderr.log").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn term_fast_exit_keeps_leader_anchored_until_descendants_are_killed() {
    let _guard = force_direct_daemon_spawn_for_test();
    let tmp = tempfile::tempdir().expect("tempdir");
    let descendant_pid_file = tmp.path().join("term-resistant-descendant.pid");
    let lifecycle_fifo_path = tmp.path().join("lifecycle.fifo");
    let mut lifecycle_fifo = FixtureFifo::create(&lifecycle_fifo_path);
    let session_dir = tmp.path().join("session-term-fast-exit");
    let command = format!(
        "sh -c 'trap \"\" TERM; printf \"%s\" \"$$\" > \"{}\"; printf D > \"{}\"; \
         while :; do sleep 1; done' & descendant=$!; \
         trap \"printf E > '{}'; exit 0\" TERM; printf L > '{}'; wait \"$descendant\"",
        descendant_pid_file.display(),
        lifecycle_fifo_path.display(),
        lifecycle_fifo_path.display(),
        lifecycle_fifo_path.display()
    );
    let config = DaemonSpawnConfig {
        session_id: "TEST_TERM_FAST_EXIT".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: write_wrapper_script(tmp.path(), "wrapper-term-fast.sh"),
        subcommand: "run".to_string(),
        args: vec!["--".to_string(), command],
        env: HashMap::new(),
    };

    let final_signal_anchor_observation = Rc::new(RefCell::new(None));
    let observer_result = Rc::clone(&final_signal_anchor_observation);
    let _observer_guard = observe_before_final_group_signal_for_test(move |leader_pid| {
        *observer_result.borrow_mut() = Some(wait_for_unreaped_child_exit(
            leader_pid,
            Duration::from_secs(1),
        ));
    });
    let mut leader_exit_proven = false;
    let cleanup_error = spawn_daemon_verified(config, |result| {
        let mut readiness = lifecycle_fifo.read_markers(2, Duration::from_secs(1))?;
        readiness.sort_unstable();
        anyhow::ensure!(
            readiness == b"DL",
            "leader and descendant sent invalid readiness markers: {readiness:?}"
        );
        // SAFETY: getpgid only inspects the positive PID returned for the
        // child owned by spawn_daemon_verified.
        let process_group = unsafe { libc::getpgid(result.pid as libc::pid_t) };
        anyhow::ensure!(
            process_group == result.pid as libc::pid_t,
            "fixture leader {} did not anchor its own process group: {process_group}",
            result.pid
        );
        // Signal only the leader so the descendant remains available to prove
        // that the subsequent anchored process-group cleanup reaches it.
        // SAFETY: result.pid identifies the child owned by spawn_daemon_verified.
        let signal_rc = unsafe { libc::kill(result.pid as libc::pid_t, libc::SIGTERM) };
        anyhow::ensure!(
            signal_rc == 0,
            "failed to signal fixture leader {}: {}",
            result.pid,
            std::io::Error::last_os_error()
        );
        anyhow::ensure!(
            lifecycle_fifo.read_markers(1, Duration::from_secs(1))? == b"E",
            "leader sent an invalid exit marker"
        );
        wait_for_unreaped_child_exit(result.pid, Duration::from_secs(1))?;
        leader_exit_proven = true;
        anyhow::bail!("force anchored cleanup")
    })
    .err()
    .expect("verification failure must clean the anchored group");

    assert!(
        leader_exit_proven,
        "the leader must be waitable but unreaped before process-group cleanup: {cleanup_error:#}; \
         stderr: {}",
        std::fs::read_to_string(session_dir.join("stderr.log"))
            .unwrap_or_else(|error| format!("<unavailable: {error}>"))
    );
    let final_signal_anchor_observation = final_signal_anchor_observation
        .borrow_mut()
        .take()
        .expect("cleanup must run the final group signal observer");
    assert!(
        final_signal_anchor_observation.is_ok(),
        "the leader must remain waitable and unreaped through the final group signal: {:#}",
        final_signal_anchor_observation
            .as_ref()
            .expect_err("failed observation must retain its error")
    );
    let descendant_pid = std::fs::read_to_string(descendant_pid_file)
        .expect("read descendant pid")
        .trim()
        .parse::<libc::pid_t>()
        .expect("parse descendant pid");
    let mut process_guard = ProcessGuard::new(descendant_pid);
    assert!(
        wait_until(Duration::from_secs(1), || !process_is_live(descendant_pid)),
        "TERM-resistant descendant {descendant_pid} survived final group SIGKILL"
    );
    process_guard.disarm();
    assert!(!session_dir.join("daemon.pid").exists());
    assert!(session_dir.join("stderr.log").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn exited_scoped_launcher_stops_unit_and_anchored_process_group() {
    let _guard = force_independent_scope_for_test();
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("session-reaped-scope");
    let descendant_pid_file = tmp.path().join("scope-descendant.pid");
    let readiness_fifo_path = tmp.path().join("scope-descendant-ready.fifo");
    let mut readiness_fifo = FixtureFifo::create(&readiness_fifo_path);
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
    let command = format!(
        "sh -c 'trap \"\" TERM; printf \"%s\" \"$$\" > \"{}\"; printf D > \"{}\"; \
         while :; do sleep 1; done' & exit 0",
        descendant_pid_file.display(),
        readiness_fifo_path.display()
    );
    let config = DaemonSpawnConfig {
        session_id: "TEST_REAPED_SCOPE".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: write_wrapper_script(tmp.path(), "wrapper-reaped-scope.sh"),
        subcommand: "run".to_string(),
        args: vec!["--".to_string(), command],
        env: HashMap::new(),
    };

    let err =
        spawn_daemon_verified_with_commands(config, &fake_systemd_run, &fake_systemctl, |result| {
            anyhow::ensure!(
                readiness_fifo.read_markers(1, Duration::from_secs(1))? == b"D",
                "scoped descendant sent an invalid readiness marker"
            );
            wait_for_unreaped_child_exit(result.pid, Duration::from_secs(1))?;
            Ok(())
        })
        .err()
        .expect("the reaped scoped launcher must be rejected");
    assert!(format!("{err:#}").contains("exited before readiness"));

    let descendant_pid = std::fs::read_to_string(descendant_pid_file)
        .expect("read descendant pid")
        .trim()
        .parse::<libc::pid_t>()
        .expect("parse descendant pid");
    let mut process_guard = ProcessGuard::new(descendant_pid);
    assert!(
        wait_until(Duration::from_secs(1), || !process_is_live(descendant_pid)),
        "anchored scoped cleanup must kill descendant {descendant_pid}"
    );
    process_guard.disarm();
    assert_eq!(
        std::fs::read_to_string(systemctl_log).expect("systemctl invocation should be logged"),
        "--user\nstop\ncsa-daemon-TEST_REAPED_SCOPE.scope\n"
    );
    assert!(!session_dir.join("daemon.pid").exists());
    assert!(!session_dir.join("daemon.scope").exists());
    assert!(session_dir.join("stderr.log").exists());
}

#[test]
fn failed_scope_spawn_uses_direct_cleanup_without_systemctl() {
    let _guard = force_independent_scope_for_test();
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("session-direct-fallback-cleanup");
    let systemctl_log = tmp.path().join("systemctl.log");
    let fake_systemctl = tmp.path().join("systemctl");
    write_executable_script(
        &fake_systemctl,
        &format!(
            "#!/bin/sh\nprintf invoked > '{}'\n",
            systemctl_log.display()
        ),
    );
    let config = DaemonSpawnConfig {
        session_id: "TEST_DIRECT_FALLBACK_CLEANUP".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: write_wrapper_script(tmp.path(), "wrapper-fallback-cleanup.sh"),
        subcommand: "run".to_string(),
        args: vec![
            "--".to_string(),
            "trap '' TERM; while :; do sleep 1; done".to_string(),
        ],
        env: HashMap::new(),
    };

    spawn_daemon_verified_with_commands(
        config,
        &tmp.path().join("missing-systemd-run"),
        &fake_systemctl,
        |_| anyhow::bail!("fallback readiness failed"),
    )
    .err()
    .expect("verification failure must fail the direct fallback spawn");

    assert!(
        !systemctl_log.exists(),
        "effective Direct fallback must not invoke systemctl"
    );
    assert!(!session_dir.join("daemon.pid").exists());
    assert!(!session_dir.join("daemon.scope").exists());
}
