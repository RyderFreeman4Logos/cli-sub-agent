use super::{InstallProvenanceStatus, inspect};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
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
    assert!(
        report
            .diagnostic()
            .contains("refusing to report installation success")
    );
    assert!(report.diagnostic().contains("will not overwrite"));
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
}

#[test]
fn install_verification_reports_unsafe_non_writable_shadow_without_overwriting_it() {
    let temp = TempDir::new().unwrap();
    let shadow = temp.path().join("shadow");
    let target_dir = temp.path().join("target");
    fs::create_dir_all(&shadow).unwrap();
    fs::create_dir_all(&target_dir).unwrap();
    let stale = write_csa(&shadow, "csa 0.1.1094 (1d04aac0)");
    fs::set_permissions(&stale, fs::Permissions::from_mode(0o555)).unwrap();
    let target = write_csa(&target_dir, "csa 0.1.1095 (3a67b06b)");

    let report = inspect(&path(&[&shadow, &target_dir]), &target, &target).unwrap();

    assert_eq!(report.status, InstallProvenanceStatus::UnsafeShadow);
    assert!(report.diagnostic().contains("not writable"));
    assert!(report.diagnostic().contains("will not overwrite"));
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
    assert!(doctor.version_output.contains("1d04aac0"));
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
