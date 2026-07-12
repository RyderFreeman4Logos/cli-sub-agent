use super::{
    InstallProvenanceStatus, NOT_EXECUTED_MISMATCH, default_intended_target, inspect, is_writable,
    version_output_with_limits,
};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::time::Duration;
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
        "#!/bin/sh\n# Ignore --version and hang forever.\nwhile true; do sleep 60; done\n",
    );

    let err = version_output_with_limits(&hanging, Duration::from_millis(250), 4096).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("timed out"),
        "expected timeout diagnostic, got: {msg}"
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
