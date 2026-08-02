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

fn corrupt_session_inventory() -> (tempfile::TempDir, crate::test_env_lock::ScopedTestEnvVar) {
    use std::fs;

    let temp = tempfile::tempdir().expect("tempdir");
    let state_home = temp.path().join("state");
    let state_home_guard =
        crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", &state_home);
    let project = temp.path().join("project");
    fs::create_dir_all(&project).expect("project");
    let session =
        csa_session::create_session(&project, Some("corrupt"), None, None).expect("create session");
    let state_path = csa_session::get_session_dir(&project, &session.meta_session_id)
        .expect("session dir")
        .join("state.toml");
    fs::write(state_path, b"not valid toml = [").expect("corrupt state");
    (temp, state_home_guard)
}

#[cfg(unix)]
#[test]
fn spawn_memory_admission_fails_closed_when_legacy_state_root_is_inaccessible() {
    let (_temp, _state_home, legacy) = inaccessible_legacy_state_root();
    let admission = build_spawn_memory_admission(Path::new("/project"), Some("current"), 1024);
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

#[test]
fn spawn_memory_admission_fails_closed_when_session_state_is_corrupt() {
    let (_temp, _state_home) = corrupt_session_inventory();

    assert!(build_spawn_memory_admission(Path::new("/project"), Some("current"), 1024).is_err());
}

#[test]
fn balloon_admission_fails_closed_when_session_state_is_corrupt() {
    let (_temp, _state_home) = corrupt_session_inventory();

    assert!(active_session_count_for_balloon(Path::new("/project"), "current").is_err());
}
