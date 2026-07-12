use super::version_output_with_limits;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn write_script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn version_probe_times_out_and_reaps_hanging_binary() {
    let temp = TempDir::new().unwrap();
    let hanging = write_script(
        temp.path(),
        "hang",
        // Ignore SIGTERM so only the SIGKILL escalation reaps the hang.
        "#!/bin/sh\ntrap '' TERM\n# Ignore --version and hang forever.\nwhile true; do sleep 60; done\n",
    );

    let start = Instant::now();
    let err = version_output_with_limits(&hanging, Duration::from_millis(250), 4096).unwrap_err();
    let elapsed = start.elapsed();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("timed out"),
        "expected timeout diagnostic, got: {msg}"
    );
    // Poll (250ms) + TERM grace (100ms) + kill-wait bound (2s) must stay finite.
    // Never allow unbounded Child::wait() to push this into multi-second hangs
    // beyond the documented kill-wait ceiling with generous scheduler slack.
    assert!(
        elapsed < Duration::from_secs(4),
        "timeout cleanup took too long (must stay within poll+kill-wait): {elapsed:?}"
    );
}

/// Direct unit: `wait_and_reap_for_exit` must return within its deadline when the
/// child remains waitable (simulates post-kill uninterruptible/unreapable leader).
#[test]
fn version_probe_reap_wait_is_deadline_bounded() {
    use std::process::{Command, Stdio};

    let mut child = Command::new("sh")
        .args(["-c", "trap '' TERM; exec sleep 30"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn unreaped sleep fixture");

    let start = Instant::now();
    let result = super::wait_and_reap_for_exit(&mut child, Duration::from_millis(200))
        .expect("try_wait poll must not error on a live child");
    let elapsed = start.elapsed();

    assert!(
        result.is_none(),
        "short-bound wait must not reap a still-running child"
    );
    assert!(
        elapsed < Duration::from_millis(800),
        "bounded try_wait loop must not hang: {elapsed:?}"
    );

    // Early-exit cleanup: SIGKILL + bounded try_wait — never unbounded wait().
    let _ = child.kill();
    let deadline = Instant::now() + FORCE_REAP_BOUND;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => break,
        }
    }
}

#[test]
fn version_probe_rejects_unbounded_output() {
    let temp = TempDir::new().unwrap();
    // Emit a fixed oversize payload then exit so the size bound is hit
    // deterministically (no race with timeout or fork pressure from hang loops).
    let spam = write_script(
        temp.path(),
        "spam",
        "#!/bin/sh\n# ~4KiB of printable output, then exit 0.\ni=0\nwhile [ \"$i\" -lt 64 ]; do\n  printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'\n  i=$((i + 1))\ndone\n",
    );

    let err = version_output_with_limits(&spam, Duration::from_secs(5), 256).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("more than"),
        "expected size-bound diagnostic, got: {msg}"
    );
    assert!(
        msg.contains("256"),
        "size bound should appear in error: {msg}"
    );
}

#[test]
fn version_probe_rejects_oversized_stderr_after_success_exit() {
    let temp = TempDir::new().unwrap();
    // Leader prints valid version on stdout but floods stderr beyond the cap.
    let spam = write_script(
        temp.path(),
        "stderr-spam",
        "#!/bin/sh\ni=0\nwhile [ \"$i\" -lt 64 ]; do\n  printf 'yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy' >&2\n  i=$((i + 1))\ndone\necho 'csa 0.1.0 (ok)'\n",
    );

    let err = version_output_with_limits(&spam, Duration::from_secs(5), 256).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("more than") && msg.contains("256"),
        "expected stderr size-bound diagnostic, got: {msg}"
    );
}

#[test]
fn version_probe_fast_exit_returns_version() {
    let temp = TempDir::new().unwrap();
    let path = write_script(
        temp.path(),
        "fast",
        "#!/bin/sh\necho 'csa 0.1.1095 (fast)'\n",
    );
    let start = Instant::now();
    let out = version_output_with_limits(&path, Duration::from_secs(2), 1024).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(out, "csa 0.1.1095 (fast)");
    assert!(
        elapsed < Duration::from_millis(500),
        "fast-exit probe should not wait full TERM grace: {elapsed:?}"
    );
}

#[test]
fn version_probe_kills_descendant_that_retains_stderr_after_leader_exit() {
    let temp = TempDir::new().unwrap();
    let pid_file = temp.path().join("descendant.pid");
    let heartbeat = temp.path().join("descendant.heartbeat");
    let pid_path = pid_file.display().to_string();
    let heartbeat_path = heartbeat.display().to_string();
    // Leader exits 0 with a valid version while a descendant keeps writing stderr
    // and a portable heartbeat file. Prior bug: Ok(Some(status)) reaped the
    // leader and never cleaned the group.
    //
    // Important: record `$!` (the background descendant), NOT `$$`.
    // POSIX `$$` is the main shell PID even inside `( ... )`, so asserting on
    // `$$` can pass after the leader is reaped while the true descendant lives.
    //
    // Liveness is proven via heartbeat growth (works on Linux and macOS). The
    // old `/proc/<pid>/stat` check returned false when `/proc` was missing, so
    // the regression passed vacuously on macOS.
    let path = write_script(
        temp.path(),
        "descendant-stderr",
        &format!(
            "#!/bin/sh\n(\n  while true; do\n    printf 'z' >&2\n    printf 'x' >> '{heartbeat_path}'\n    sleep 0.05\n  done\n) &\necho $! > '{pid_path}'\necho 'csa 0.1.0 (leader-ok)'\nexit 0\n"
        ),
    );

    let out = version_output_with_limits(&path, Duration::from_secs(2), 64 * 1024).unwrap();
    assert_eq!(out, "csa 0.1.0 (leader-ok)");

    // Marker is written by the leader before exit; content survives on disk even
    // after group cleanup. Missing/invalid PID is a hard failure — never treat
    // "no evidence" as success (that made the macOS path vacuous).
    let body = fs::read_to_string(&pid_file).unwrap_or_default();
    let pid = body
        .trim()
        .parse::<i32>()
        .unwrap_or_else(|_| panic!("descendant pid marker must be a valid pid, got {body:?}"));
    assert!(
        pid > 1,
        "descendant pid marker must be written so the regression is not vacuous (got {body:?})"
    );

    // Portable observable: after cleanup, heartbeat growth must stop across
    // several former write intervals. Allow the OS a moment to apply SIGKILL.
    std::thread::sleep(Duration::from_millis(150));
    let size1 = fs::metadata(&heartbeat).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(Duration::from_millis(250));
    let size2 = fs::metadata(&heartbeat).map(|m| m.len()).unwrap_or(0);
    assert_eq!(
        size1, size2,
        "descendant pid {pid} heartbeat must stop growing after version probe \
         (size1={size1}, size2={size2})"
    );

    // On Linux, additionally assert the task table entry is gone or a zombie.
    #[cfg(target_os = "linux")]
    {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut still_alive = true;
        while Instant::now() < deadline {
            if !live_non_zombie_process(pid) {
                still_alive = false;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !still_alive,
            "descendant pid {pid} that retained stderr must be gone or reaped after version probe"
        );
    }
}

#[path = "install_provenance_probe_hang_group.rs"]
mod hang_group;
use hang_group::*;

/// Round-5 HIGH: wait-state unknown must not emit PID or negative-PGID signals.
///
/// After wait-state loss (ECHILD / consumed status), `finish` and cleanup must
/// leave the hanging leader alive — proving no SIGTERM/SIGKILL was sent to the
/// cached PID or negative PGID.
///
/// Use `sh -c` (not a freshly-written executable script) to avoid ETXTBSY under
/// parallel cargo test when the kernel still has the path open for write.
#[test]
fn version_probe_ownership_unknown_emits_no_pid_or_pgid_signal() {
    let temp = TempDir::new().unwrap();
    let (child, cleanup) = spawn_hang_group_with_descendant(temp.path());
    let pid = cleanup.leader();
    let descendant = cleanup.descendant();

    let mut session = super::VersionProbeSession::new(child);
    // Simulate poll_version_probe's waitid failure path.
    session.mark_ownership_unknown();
    let stop =
        super::ProbeStopReason::OwnershipUnknown(std::io::Error::from_raw_os_error(libc::ECHILD));
    let finish_err = session
        .finish(&stop)
        .expect_err("ownership-unknown must refuse signal/reap");
    let msg = format!("{finish_err:#}");
    assert!(
        msg.contains("cannot safely signal or reap"),
        "expected ownership-unknown diagnostic, got: {msg}"
    );

    // Critical assertion: fixture must still be alive — no PID/negative-PGID kill.
    assert!(
        process_exists(pid) && process_exists(descendant),
        "ownership-unknown cleanup must not SIGTERM/SIGKILL pid {pid} or descendant {descendant}"
    );

    // Happy path: explicit cleanup + identity-safe assert; Drop disarmed.
    // On any panic above, HangGroupCleanup::Drop still group-kills the fixture.
    cleanup.force_kill_reap_and_assert();
}

/// Drop path after wait-state loss must also refuse group signals.
#[test]
fn version_probe_drop_after_ownership_unknown_does_not_signal() {
    let temp = TempDir::new().unwrap();
    let (child, cleanup) = spawn_hang_group_with_descendant(temp.path());
    let pid = cleanup.leader();
    let descendant = cleanup.descendant();

    {
        let mut session = super::VersionProbeSession::new(child);
        session.mark_ownership_unknown();
        // Drop without finish — must not signal.
    }

    assert!(
        process_exists(pid) && process_exists(descendant),
        "Drop after ownership-unknown must not kill pid {pid} or descendant {descendant}"
    );
    cleanup.force_kill_reap_and_assert();
}

/// Output-stat errors still own the leader: cleanup must kill the process group.
#[test]
fn version_probe_output_stat_error_still_cleans_owned_group() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let child = Command::new("sh")
        .args(["-c", "trap '' TERM; while true; do sleep 60; done"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn hang fixture");
    let pid = child.id();
    assert!(process_exists(pid));

    let session = super::VersionProbeSession::new(child);
    let stop = super::ProbeStopReason::OutputStatError(std::io::Error::other("stat failed"));
    // finish should terminate the owned group then reap (or timeout-reap).
    let _ = session.finish(&stop);

    // Allow SIGKILL to apply.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && process_exists(pid) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_exists(pid),
        "OutputStatError must still clean the owned process group (pid {pid} still exists)"
    );
}

/// End-to-end poll path: inject ECHILD into waitid and assert cleanup never
/// signals the still-live process group (PID known from spawn, not a racey file).
#[test]
fn version_probe_waitid_echild_does_not_signal_process_group() {
    use super::test_hooks;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let _guard = test_hooks::HOOK_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    test_hooks::clear();

    let temp = TempDir::new().unwrap();
    // Long-lived append-heartbeat descendant so RAII cleanup can assert growth stop.
    let pid_file = temp.path().join("descendant.pid");
    let hb = temp.path().join("descendant.hb");
    let hanging = write_script(
        temp.path(),
        "hang-echild",
        &format!(
            "#!/bin/sh\n\
             trap '' TERM\n\
             (while true; do printf 'x' >>'{hb}'; sleep 0.05; done) &\n\
             echo $! >'{pid}'\n\
             while true; do sleep 60; done\n",
            hb = hb.display(),
            pid = pid_file.display(),
        ),
    );

    let stdout = tempfile::tempfile().expect("stdout tempfile");
    let stderr = tempfile::tempfile().expect("stderr tempfile");

    let mut cmd = Command::new(&hanging);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout.try_clone().unwrap()))
        .stderr(Stdio::from(stderr.try_clone().unwrap()))
        .process_group(0);

    let child = cmd.spawn().expect("spawn hang fixture");
    let pid = child.id();
    // Arm RAII immediately so assert/poll panics cannot leak the hang group.
    let mut cleanup = HangGroupCleanup::new(pid, 0, hb);

    let deadline = Instant::now() + FORCE_REAP_BOUND;
    let mut descendant = 0u32;
    while Instant::now() < deadline {
        if let Ok(s) = fs::read_to_string(&pid_file) {
            if let Ok(p) = s.trim().parse::<u32>() {
                if p > 1 {
                    descendant = p;
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        descendant > 1 && process_exists(pid) && process_exists(descendant),
        "fixture leader and descendant must start before waitid inject"
    );
    cleanup.bind_descendant(descendant);

    // Inject ECHILD on the next waitid call inside the poll loop.
    test_hooks::force_waitid_once(Err(std::io::Error::from_raw_os_error(libc::ECHILD)));

    let mut session = super::VersionProbeSession::new(child);
    let stop =
        super::poll_version_probe(&mut session, &stdout, &stderr, Duration::from_secs(2), 4096);
    assert!(
        matches!(stop, super::ProbeStopReason::OwnershipUnknown(_)),
        "waitid ECHILD must surface as OwnershipUnknown, got {stop:?}"
    );
    assert!(
        session.ownership_unknown,
        "probe must mark ownership_unknown before returning"
    );

    // Same production cleanup order as version_output_with_limits.
    session.abandon_without_signaling();

    assert!(
        process_exists(pid) && process_exists(descendant),
        "waitid ECHILD path must not signal pid/pgid {pid} or descendant {descendant}"
    );

    cleanup.force_kill_reap_and_assert();
    test_hooks::clear();
}
