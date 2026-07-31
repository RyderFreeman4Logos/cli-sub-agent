use super::*;

#[cfg(unix)]
fn inaccessible_legacy_state_root() -> (
    tempfile::TempDir,
    crate::test_env_lock::ScopedTestEnvVar,
    std::path::PathBuf,
) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let state_home = temp.path().join("state");
    let state_home_guard =
        crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", &state_home);
    let legacy = state_home.join("csa");
    fs::create_dir_all(&legacy).expect("legacy state root");
    let mut permissions = fs::metadata(&legacy)
        .expect("legacy metadata")
        .permissions();
    permissions.set_mode(0o000);
    fs::set_permissions(&legacy, permissions).expect("block legacy state root");
    (temp, state_home_guard, legacy)
}

#[cfg(unix)]
fn restore_state_root_permissions(path: &Path) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("restore legacy state-root permissions");
}

#[cfg(unix)]
#[test]
fn spawn_memory_admission_fails_closed_when_legacy_state_root_is_inaccessible() {
    let (_temp, _state_home, legacy) = inaccessible_legacy_state_root();
    let admission = build_spawn_memory_admission(Path::new("/project"), "current", 1024);
    restore_state_root_permissions(&legacy);

    assert!(
        admission.is_err(),
        "inaccessible state inventory must block spawn-memory admission, not yield zero active memory"
    );
}

#[cfg(unix)]
#[test]
fn balloon_admission_fails_closed_when_legacy_state_root_is_inaccessible() {
    let (_temp, _state_home, legacy) = inaccessible_legacy_state_root();
    let count = active_session_count_for_balloon(Path::new("/project"), "current");
    restore_state_root_permissions(&legacy);

    assert!(
        count.is_err(),
        "inaccessible state inventory must block balloon admission, not yield zero active sessions"
    );
}
