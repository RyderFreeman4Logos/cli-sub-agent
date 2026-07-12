use super::{
    InstallProvenanceStatus, NOT_EXECUTED_MISMATCH, default_intended_target, inspect, inspect_os,
    is_writable, version_output_with_limits,
};
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn write_csa(dir: &Path, version: &str) -> std::path::PathBuf {
    let path = dir.join("csa");
    fs::write(
        &path,
        format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '{version}'; fi\n"),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Shadow that would leave a side-effect marker if ever executed.
fn write_marker_shadow(dir: &Path, marker: &Path) -> std::path::PathBuf {
    let path = dir.join("csa");
    let marker_display = marker.display();
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf 'ran\\n' >'{marker_display}'\nif [ \"$1\" = \"--version\" ]; then echo 'csa 0.0.0 (evil)'; fi\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn write_script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn path(entries: &[&Path]) -> String {
    std::env::join_paths(entries)
        .unwrap()
        .into_string()
        .unwrap()
}

#[test]
fn install_verification_rejects_stale_higher_priority_path_shadow() {
    let temp = TempDir::new().unwrap();
    let shadow = temp.path().join("shadow");
    let target_dir = temp.path().join("target");
    fs::create_dir_all(&shadow).unwrap();
    fs::create_dir_all(&target_dir).unwrap();
    let stale = write_csa(&shadow, "csa 0.1.1094 (1d04aac0)");
    let target = write_csa(&target_dir, "csa 0.1.1095 (3a67b06b)");

    let report = inspect(&path(&[&shadow, &target_dir]), &target, &target).unwrap();

    assert_eq!(report.status, InstallProvenanceStatus::StaleShadow);
    assert_eq!(report.path_resolved, stale);
    assert_eq!(report.version_output, NOT_EXECUTED_MISMATCH);
    assert!(
        report
            .diagnostic()
            .contains("refusing to report installation success")
    );
    assert!(report.diagnostic().contains("will not overwrite"));
}

#[test]
fn install_verification_never_executes_mismatched_path_shadow() {
    let temp = TempDir::new().unwrap();
    let shadow = temp.path().join("shadow");
    let target_dir = temp.path().join("target");
    fs::create_dir_all(&shadow).unwrap();
    fs::create_dir_all(&target_dir).unwrap();
    let marker = temp.path().join("shadow-was-executed");
    let stale = write_marker_shadow(&shadow, &marker);
    let target = write_csa(&target_dir, "csa 0.1.1095 (3a67b06b)");

    let report = inspect(&path(&[&shadow, &target_dir]), &target, &target).unwrap();

    assert_eq!(report.status, InstallProvenanceStatus::StaleShadow);
    assert_eq!(report.path_resolved, stale);
    assert_eq!(report.version_output, NOT_EXECUTED_MISMATCH);
    assert!(
        !marker.exists(),
        "mismatched PATH shadow must not be executed for diagnostics"
    );
    assert!(!report.is_current());
}

#[test]
fn install_verification_accepts_unshadowed_exact_artifact() {
    let temp = TempDir::new().unwrap();
    let target_dir = temp.path().join("target");
    fs::create_dir_all(&target_dir).unwrap();
    let target = write_csa(&target_dir, "csa 0.1.1095 (3a67b06b)");

    let report = inspect(&path(&[&target_dir]), &target, &target).unwrap();

    assert_eq!(report.status, InstallProvenanceStatus::Current);
    assert_eq!(report.path_resolved, target);
    assert_eq!(report.version_output, "csa 0.1.1095 (3a67b06b)");
    assert_eq!(report.artifact_version, report.version_output);
}

#[test]
fn install_verification_accepts_matching_duplicate_path_entry() {
    let temp = TempDir::new().unwrap();
    let first = temp.path().join("first");
    let target_dir = temp.path().join("target");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&target_dir).unwrap();
    let target = write_csa(&target_dir, "csa 0.1.1095 (3a67b06b)");
    symlink(&target, first.join("csa")).unwrap();

    let report = inspect(&path(&[&first, &target_dir]), &target, &target).unwrap();

    assert_eq!(report.status, InstallProvenanceStatus::Current);
    assert_ne!(report.path_resolved, target);
    // Bytes match → artifact version reused; shadow not re-executed.
    assert_eq!(report.version_output, "csa 0.1.1095 (3a67b06b)");
}

#[test]
fn install_verification_reports_unsafe_non_writable_shadow_without_overwriting_it() {
    let temp = TempDir::new().unwrap();
    let shadow = temp.path().join("shadow");
    let target_dir = temp.path().join("target");
    fs::create_dir_all(&shadow).unwrap();
    fs::create_dir_all(&target_dir).unwrap();
    let marker = temp.path().join("unsafe-shadow-ran");
    let stale = write_marker_shadow(&shadow, &marker);
    fs::set_permissions(&stale, fs::Permissions::from_mode(0o555)).unwrap();
    let target = write_csa(&target_dir, "csa 0.1.1095 (3a67b06b)");

    let report = inspect(&path(&[&shadow, &target_dir]), &target, &target).unwrap();

    assert_eq!(report.status, InstallProvenanceStatus::UnsafeShadow);
    assert_eq!(report.version_output, NOT_EXECUTED_MISMATCH);
    assert!(!marker.exists(), "unsafe shadow must not be executed");
    assert!(report.diagnostic().contains("not writable"));
    assert!(report.diagnostic().contains("will not overwrite"));
}

#[test]
fn is_writable_uses_effective_access_not_mere_mode_bits() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("shadow");
    fs::write(&path, "x").unwrap();

    // Owner-write mode: current process owns the file → effectively writable.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        is_writable(&path).unwrap(),
        "owner-writable file should be effectively writable"
    );

    // No write bits at all → not effectively writable (and mode & 0o222 == 0).
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    assert!(
        !is_writable(&path).unwrap(),
        "read-only file must not be classified as writable"
    );

    // 0555: no write bit for anyone — effective access fails.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).unwrap();
    assert!(
        !is_writable(&path).unwrap(),
        "0555 must not be effectively writable"
    );

    // Restore for TempDir cleanup.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
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
    let msg = err.to_string();
    assert!(
        msg.contains("timed out"),
        "expected timeout diagnostic, got: {msg}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "timeout cleanup took too long: {elapsed:?}"
    );
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
    let pid_path = pid_file.display().to_string();
    // Leader exits 0 with a valid version while a descendant keeps writing stderr.
    // Prior bug: Ok(Some(status)) reaped the leader and never cleaned the group.
    //
    // Important: record `$!` (the background descendant), NOT `$$`.
    // POSIX `$$` is the main shell PID even inside `( ... )`, so asserting on
    // `$$` can pass after the leader is reaped while the true descendant lives.
    let path = write_script(
        temp.path(),
        "descendant-stderr",
        &format!(
            "#!/bin/sh\n(\n  while true; do\n    printf 'z' >&2\n    sleep 0.05\n  done\n) &\necho $! > '{pid_path}'\necho 'csa 0.1.0 (leader-ok)'\nexit 0\n"
        ),
    );

    let out = version_output_with_limits(&path, Duration::from_secs(2), 64 * 1024).unwrap();
    assert_eq!(out, "csa 0.1.0 (leader-ok)");

    // Marker is written by the leader before exit; content survives on disk even
    // after group cleanup. Liveness uses /proc state (not kill(0)) so zombies are
    // not treated as surviving writers.
    let body = fs::read_to_string(&pid_file).unwrap_or_default();
    let pid = body.trim().parse::<i32>().unwrap_or(0);
    if pid <= 1 {
        // Descendant may have been killed before the leader flushed the marker.
        return;
    }

    // Allow the OS a moment to apply SIGKILL / reap.
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

/// True if `/proc/<pid>` exists in a non-zombie state (running/sleeping/etc.).
///
/// Prefer this over `kill(pid, 0)`: zombies still "exist" for kill(0) but are not
/// live writers, and pure existence checks cannot distinguish PID reuse without
/// `/proc` starttime — here "still alive" means a non-zombie task table entry.
fn live_non_zombie_process(pid: i32) -> bool {
    if pid <= 1 {
        return false;
    }
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // Format: pid (comm) state ppid ...
    let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else {
        return false;
    };
    let Some(state) = rest
        .split_whitespace()
        .next()
        .and_then(|s| s.chars().next())
    else {
        return false;
    };
    state != 'Z'
}

#[test]
fn install_verification_detects_stale_shadow_in_non_utf8_path_component() {
    // A higher-priority PATH directory whose name is not valid UTF-8 must still
    // participate in resolution. Lossy conversion to String would replace the
    // invalid bytes with U+FFFD and skip the real shadow directory.
    let temp = TempDir::new().unwrap();
    let mut shadow_bytes = temp.path().as_os_str().as_bytes().to_vec();
    shadow_bytes.push(b'/');
    shadow_bytes.extend_from_slice(b"sha\xffdow");
    let shadow = PathBuf::from(OsString::from_vec(shadow_bytes));
    let target_dir = temp.path().join("target");
    fs::create_dir_all(&shadow).unwrap();
    fs::create_dir_all(&target_dir).unwrap();

    let stale = write_csa(&shadow, "csa 0.1.1094 (non-utf8-shadow)");
    let target = write_csa(&target_dir, "csa 0.1.1095 (fresh)");

    let path_os = std::env::join_paths([&shadow, &target_dir]).unwrap();
    let report = inspect_os(path_os.as_os_str(), &target, &target).unwrap();

    assert_eq!(
        report.status,
        InstallProvenanceStatus::StaleShadow,
        "non-UTF-8 higher-priority PATH dir must still resolve the stale shadow"
    );
    assert_eq!(report.path_resolved, stale);
    assert_eq!(report.version_output, NOT_EXECUTED_MISMATCH);
    assert!(!report.is_current());
}

#[test]
fn path_lookup_skips_owner_unexecutable_mode_bits() {
    // 0401: owner-read + other-execute. Mode-bit heuristics wrongly accept this;
    // execvp / access(X_OK) as the owner do not.
    let temp = TempDir::new().unwrap();
    let bad = temp.path().join("early");
    let good = temp.path().join("late");
    fs::create_dir_all(&bad).unwrap();
    fs::create_dir_all(&good).unwrap();
    let bad_bin = bad.join("csa");
    fs::write(&bad_bin, b"#!/bin/sh\necho should-not-run\n").unwrap();
    fs::set_permissions(&bad_bin, fs::Permissions::from_mode(0o401)).unwrap();
    let good_bin = write_csa(&good, "csa 0.1.0 (good)");

    let report = inspect(&path(&[&bad, &good]), &good_bin, &good_bin).unwrap();
    assert_eq!(report.path_resolved, good_bin);
    assert_eq!(report.status, InstallProvenanceStatus::Current);
}

#[test]
fn doctor_and_install_share_the_same_report() {
    let temp = TempDir::new().unwrap();
    let shadow = temp.path().join("shadow");
    let target_dir = temp.path().join("target");
    fs::create_dir_all(&shadow).unwrap();
    fs::create_dir_all(&target_dir).unwrap();
    write_csa(&shadow, "csa 0.1.1094 (1d04aac0)");
    let target = write_csa(&target_dir, "csa 0.1.1095 (3a67b06b)");

    let install = inspect(&path(&[&shadow, &target_dir]), &target, &target).unwrap();
    let doctor = inspect(&path(&[&shadow, &target_dir]), &target, &target).unwrap();

    assert_eq!(install, doctor);
    assert_eq!(doctor.version_output, NOT_EXECUTED_MISMATCH);
    assert_eq!(doctor.resolved_hash != doctor.artifact_hash, true);
}

#[test]
fn install_report_json_is_stable_and_additive() {
    let temp = TempDir::new().unwrap();
    let target_dir = temp.path().join("target");
    fs::create_dir_all(&target_dir).unwrap();
    let target = write_csa(&target_dir, "csa 0.1.1095 (3a67b06b)");
    let report = inspect(&path(&[&target_dir]), &target, &target).unwrap();
    let json = report.to_json();
    assert_eq!(json["status"], "current");
    assert_eq!(json["current"], true);
    assert!(json["artifact_sha256"].as_str().is_some());
    assert!(json["path_resolved"].as_str().is_some());
    assert!(json["intended_target"].as_str().is_some());
}

#[test]
fn install_verification_errors_when_csa_missing_from_path() {
    let temp = TempDir::new().unwrap();
    let empty = temp.path().join("empty");
    let target_dir = temp.path().join("target");
    fs::create_dir_all(&empty).unwrap();
    fs::create_dir_all(&target_dir).unwrap();
    let target = write_csa(&target_dir, "csa 0.1.1095 (3a67b06b)");

    let err = inspect(&path(&[&empty]), &target, &target).unwrap_err();
    assert!(
        err.to_string().contains("could not resolve"),
        "missing PATH executable should fail closed: {err}"
    );
}

#[test]
fn install_verification_handles_spaces_in_path_entries() {
    let temp = TempDir::new().unwrap();
    let shadow = temp.path().join("shadow with spaces");
    let target_dir = temp.path().join("target dir");
    fs::create_dir_all(&shadow).unwrap();
    fs::create_dir_all(&target_dir).unwrap();
    let stale = write_csa(&shadow, "csa 0.1.1094 (stale)");
    let target = write_csa(&target_dir, "csa 0.1.1095 (fresh)");

    let report = inspect(&path(&[&shadow, &target_dir]), &target, &target).unwrap();

    assert_eq!(report.status, InstallProvenanceStatus::StaleShadow);
    assert_eq!(report.path_resolved, stale);
    assert_eq!(report.version_output, NOT_EXECUTED_MISMATCH);
}

#[test]
fn install_verification_ignores_non_executable_name_collision() {
    let temp = TempDir::new().unwrap();
    let first = temp.path().join("first");
    let target_dir = temp.path().join("target");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&target_dir).unwrap();
    let non_exec = first.join("csa");
    fs::write(&non_exec, "#!/bin/sh\necho not-exec\n").unwrap();
    fs::set_permissions(&non_exec, fs::Permissions::from_mode(0o644)).unwrap();
    let target = write_csa(&target_dir, "csa 0.1.1095 (3a67b06b)");

    let report = inspect(&path(&[&first, &target_dir]), &target, &target).unwrap();

    assert_eq!(report.status, InstallProvenanceStatus::Current);
    assert_eq!(report.path_resolved, target);
}

#[test]
fn default_intended_target_is_unix_usr_local_bin() {
    assert_eq!(default_intended_target(), Path::new("/usr/local/bin/csa"));
}
