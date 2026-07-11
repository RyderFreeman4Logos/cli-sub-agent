use super::*;

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
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
    let session_dir = tmp.path().join("session-descendant-cleanup");
    let command = format!(
        "(trap '' TERM; while :; do sleep 1; done) & echo $! > '{}'; exit 0",
        descendant_pid_file.display()
    );
    let config = DaemonSpawnConfig {
        session_id: "TEST_EARLY_EXIT_DESCENDANT".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: write_wrapper_script(tmp.path(), "wrapper-descendant.sh"),
        subcommand: "run".to_string(),
        args: vec!["--".to_string(), command],
        env: HashMap::new(),
    };

    let err = spawn_daemon_verified(config, |_| {
        anyhow::ensure!(
            wait_until(Duration::from_secs(1), || descendant_pid_file.exists()),
            "descendant fixture did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
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
    let ready_file = tmp.path().join("leader.ready");
    let descendant_pid_file = tmp.path().join("term-resistant-descendant.pid");
    let monitor_ready_file = tmp.path().join("anchor-monitor.ready");
    let anchor_observation_file = tmp.path().join("leader-anchor.state");
    let session_dir = tmp.path().join("session-term-fast-exit");
    let command = format!(
        "leader=$$; (trap '' TERM; printf ready > '{}'; while read -r stat < \
         /proc/$leader/stat; do case \"$stat\" in *\") Z \"*) printf zombie > '{}' ;; esac; \
         done; printf missing > '{}') & echo $! > '{}'; trap 'exit 0' TERM; printf ready > \
         '{}'; while :; do sleep 1; done",
        monitor_ready_file.display(),
        anchor_observation_file.display(),
        anchor_observation_file.display(),
        descendant_pid_file.display(),
        ready_file.display()
    );
    let config = DaemonSpawnConfig {
        session_id: "TEST_TERM_FAST_EXIT".to_string(),
        session_dir: session_dir.clone(),
        csa_binary: write_wrapper_script(tmp.path(), "wrapper-term-fast.sh"),
        subcommand: "run".to_string(),
        args: vec!["--".to_string(), command],
        env: HashMap::new(),
    };

    spawn_daemon_verified(config, |_| {
        anyhow::ensure!(
            wait_until(Duration::from_secs(1), || {
                ready_file.exists() && monitor_ready_file.exists()
            }),
            "TERM-fast fixture and anchor monitor did not become ready"
        );
        anyhow::bail!("force anchored cleanup")
    })
    .err()
    .expect("verification failure must clean the anchored group");

    assert_eq!(
        std::fs::read_to_string(&anchor_observation_file)
            .expect("TERM-resistant descendant should observe leader state"),
        "zombie",
        "the leader must remain an unreaped zombie until the final group signal"
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
        "(trap '' TERM; while :; do sleep 1; done) & echo $! > '{}'; exit 0",
        descendant_pid_file.display()
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
        spawn_daemon_verified_with_commands(config, &fake_systemd_run, &fake_systemctl, |_| {
            anyhow::ensure!(
                wait_until(Duration::from_secs(1), || descendant_pid_file.exists()),
                "scoped descendant fixture did not start"
            );
            std::thread::sleep(Duration::from_millis(50));
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
