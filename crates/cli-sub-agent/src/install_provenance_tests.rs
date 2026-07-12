use super::{
    InstallProvenanceStatus, NOT_EXECUTED_MISMATCH, default_intended_target, inspect, inspect_os,
    is_writable,
};
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
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

fn path(entries: &[&Path]) -> String {
    std::env::join_paths(entries)
        .unwrap()
        .into_string()
        .unwrap()
}

/// Whether the current process is effectively root (UID 0). Root / CAP_DAC_OVERRIDE
/// retains write and can execute any file with any execute bit — DAC-dependent
/// tests must be privilege-aware so privileged containers stay green.
fn is_effective_root() -> bool {
    // SAFETY: geteuid has no preconditions and returns the caller's euid.
    unsafe { libc::geteuid() == 0 }
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

    // Hash mismatch + no execute of untrusted shadow either way.
    assert_eq!(report.version_output, NOT_EXECUTED_MISMATCH);
    assert!(!marker.exists(), "unsafe shadow must not be executed");

    if is_effective_root() {
        // Root retains write via DAC override; production correctly classifies
        // the shadow as writable StaleShadow (would overwrite) not UnsafeShadow.
        assert_eq!(report.status, InstallProvenanceStatus::StaleShadow);
        assert!(is_writable(&stale).unwrap());
    } else {
        assert_eq!(report.status, InstallProvenanceStatus::UnsafeShadow);
        assert!(report.diagnostic().contains("not writable"));
        assert!(report.diagnostic().contains("will not overwrite"));
    }
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

    // No write bits at all — unprivileged callers fail; root retains write.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    if is_effective_root() {
        assert!(
            is_writable(&path).unwrap(),
            "root retains write on 0444 via DAC override (access W_OK)"
        );
    } else {
        assert!(
            !is_writable(&path).unwrap(),
            "read-only file must not be classified as writable"
        );
    }

    // 0555: no write bit for anyone — same privilege split.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).unwrap();
    if is_effective_root() {
        assert!(
            is_writable(&path).unwrap(),
            "root retains write on 0555 via DAC override (access W_OK)"
        );
    } else {
        assert!(
            !is_writable(&path).unwrap(),
            "0555 must not be effectively writable"
        );
    }

    // Restore for TempDir cleanup.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
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
    // execvp / access(X_OK) as the unprivileged owner do not. Root can execute
    // any file that has any execute bit, so resolution is privilege-aware.
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
    if is_effective_root() {
        // Root X_OK succeeds on 0401; hash mismatch → StaleShadow (root can write).
        assert_eq!(report.path_resolved, bad_bin);
        assert_eq!(report.status, InstallProvenanceStatus::StaleShadow);
        assert_eq!(report.version_output, NOT_EXECUTED_MISMATCH);
    } else {
        assert_eq!(report.path_resolved, good_bin);
        assert_eq!(report.status, InstallProvenanceStatus::Current);
    }
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
    assert!(doctor.resolved_hash != doctor.artifact_hash);
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
