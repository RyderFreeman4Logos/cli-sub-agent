use std::sync::{LazyLock, Mutex};

static GEMINI_RUNTIME_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct ScopedEnvVar {
    key: &'static str,
    original: Option<String>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by GEMINI_RUNTIME_ENV_LOCK.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }

    fn unset(key: &'static str) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by GEMINI_RUNTIME_ENV_LOCK.
        unsafe { std::env::remove_var(key) };
        Self { key, original }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation guarded by GEMINI_RUNTIME_ENV_LOCK.
        unsafe {
            match self.original.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn create_project_with_mise(base: &Path, name: &str) -> (PathBuf, PathBuf) {
    let project_root = base.join(name);
    fs::create_dir_all(&project_root).expect("create project root");
    let mise_toml_path = project_root.join("mise.toml");
    fs::write(&mise_toml_path, "[tools]\nnode = \"20\"\n").expect("write mise.toml");
    (project_root, mise_toml_path)
}

#[test]
fn probe_host_mise_trust_db_returns_project_mise_toml_when_project_root_is_trusted() {
    let _env_guard = GEMINI_RUNTIME_ENV_LOCK.lock().expect("env lock");
    let temp = tempfile::tempdir().expect("tempdir");
    let state_home = temp.path().join("state-home");
    let trust_db_dir = state_home.join(GEMINI_HOST_MISE_TRUST_DB_RELATIVE_PATH);
    let (project_root, mise_toml_path) = create_project_with_mise(temp.path(), "project");
    fs::create_dir_all(&trust_db_dir).expect("create trust db dir");
    std::os::unix::fs::symlink(&project_root, trust_db_dir.join("trusted-project"))
        .expect("create trusted project symlink");

    let _xdg_state_home = ScopedEnvVar::set("XDG_STATE_HOME", &state_home.to_string_lossy());
    let _trusted_paths = ScopedEnvVar::unset("MISE_TRUSTED_CONFIG_PATHS");

    assert_eq!(
        probe_host_mise_trust_db(&project_root, &mise_toml_path),
        Some(canonicalize_if_exists(&mise_toml_path))
    );
}

#[test]
fn probe_host_mise_trust_db_rejects_non_matching_symlink_targets() {
    let _env_guard = GEMINI_RUNTIME_ENV_LOCK.lock().expect("env lock");
    let temp = tempfile::tempdir().expect("tempdir");
    let state_home = temp.path().join("state-home");
    let trust_db_dir = state_home.join(GEMINI_HOST_MISE_TRUST_DB_RELATIVE_PATH);
    let (project_root, mise_toml_path) = create_project_with_mise(temp.path(), "project");
    let other_root = temp.path().join("other-project");
    fs::create_dir_all(&other_root).expect("create other project root");
    fs::create_dir_all(&trust_db_dir).expect("create trust db dir");
    std::os::unix::fs::symlink(&other_root, trust_db_dir.join("trusted-other-project"))
        .expect("create mismatched trusted project symlink");

    let _xdg_state_home = ScopedEnvVar::set("XDG_STATE_HOME", &state_home.to_string_lossy());
    let _trusted_paths = ScopedEnvVar::unset("MISE_TRUSTED_CONFIG_PATHS");

    assert_eq!(probe_host_mise_trust_db(&project_root, &mise_toml_path), None);
}

#[test]
fn probe_host_mise_trust_db_skips_missing_non_symlink_and_broken_entries() {
    let _env_guard = GEMINI_RUNTIME_ENV_LOCK.lock().expect("env lock");
    let temp = tempfile::tempdir().expect("tempdir");
    let state_home = temp.path().join("state-home");
    let trust_db_dir = state_home.join(GEMINI_HOST_MISE_TRUST_DB_RELATIVE_PATH);
    let (project_root, mise_toml_path) = create_project_with_mise(temp.path(), "project");

    let _xdg_state_home = ScopedEnvVar::set("XDG_STATE_HOME", &state_home.to_string_lossy());
    let _trusted_paths = ScopedEnvVar::unset("MISE_TRUSTED_CONFIG_PATHS");

    assert_eq!(
        probe_host_mise_trust_db(&project_root, &mise_toml_path),
        None,
        "missing trust DB dir should be treated as untrusted"
    );

    fs::create_dir_all(&trust_db_dir).expect("create trust db dir");
    fs::write(trust_db_dir.join("plain-file"), "not a symlink").expect("write plain file");
    std::os::unix::fs::symlink(
        temp.path().join("missing-project-root"),
        trust_db_dir.join("broken-link"),
    )
    .expect("create broken symlink");

    assert_eq!(
        probe_host_mise_trust_db(&project_root, &mise_toml_path),
        None,
        "invalid trust DB entries should be ignored"
    );
}

#[test]
fn prepare_gemini_acp_runtime_synthesizes_mise_trusted_config_paths_from_host_db() {
    let _env_guard = GEMINI_RUNTIME_ENV_LOCK.lock().expect("env lock");
    let temp = tempfile::tempdir().expect("tempdir");
    let state_home = temp.path().join("state-home");
    let trust_db_dir = state_home.join(GEMINI_HOST_MISE_TRUST_DB_RELATIVE_PATH);
    let (project_root, mise_toml_path) = create_project_with_mise(temp.path(), "project");
    let source_home = temp.path().join("source-home");
    let session_id = "01TESTGEMINIMISETRUSTDB000000001";
    fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");
    fs::create_dir_all(&trust_db_dir).expect("create trust db dir");
    std::os::unix::fs::symlink(&project_root, trust_db_dir.join("trusted-project"))
        .expect("create trusted project symlink");

    let _xdg_state_home = ScopedEnvVar::set("XDG_STATE_HOME", &state_home.to_string_lossy());
    let _trusted_paths = ScopedEnvVar::unset("MISE_TRUSTED_CONFIG_PATHS");

    let mut env = HashMap::new();
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );

    prepare_gemini_acp_runtime(
        &mut env,
        Some(project_root.as_path()),
        None,
        session_id,
        &["--acp".to_string()],
    )
    .expect("prepare runtime");

    assert_eq!(
        env.get("MISE_TRUSTED_CONFIG_PATHS"),
        Some(&canonicalize_if_exists(&mise_toml_path).to_string_lossy().into_owned()),
        "runtime should synthesize trust only from the host mise DB when the user already trusted this project"
    );
}

#[test]
fn prepare_gemini_acp_runtime_prefers_process_mise_trusted_config_paths_over_host_db() {
    let _env_guard = GEMINI_RUNTIME_ENV_LOCK.lock().expect("env lock");
    let temp = tempfile::tempdir().expect("tempdir");
    let state_home = temp.path().join("state-home");
    let trust_db_dir = state_home.join(GEMINI_HOST_MISE_TRUST_DB_RELATIVE_PATH);
    let (project_root, _mise_toml_path) = create_project_with_mise(temp.path(), "project");
    let source_home = temp.path().join("source-home");
    let session_id = "01TESTGEMINIMISETRUSTPROC0000001";
    fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");
    fs::create_dir_all(&trust_db_dir).expect("create trust db dir");
    std::os::unix::fs::symlink(&project_root, trust_db_dir.join("trusted-project"))
        .expect("create trusted project symlink");

    let _xdg_state_home = ScopedEnvVar::set("XDG_STATE_HOME", &state_home.to_string_lossy());
    let _trusted_paths = ScopedEnvVar::set(
        "MISE_TRUSTED_CONFIG_PATHS",
        "/tmp/process-env-trusted.mise.toml",
    );

    let mut env = HashMap::new();
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );

    prepare_gemini_acp_runtime(
        &mut env,
        Some(project_root.as_path()),
        None,
        session_id,
        &["--acp".to_string()],
    )
    .expect("prepare runtime");

    assert_eq!(
        env.get("MISE_TRUSTED_CONFIG_PATHS"),
        Some(&"/tmp/process-env-trusted.mise.toml".to_string()),
        "an explicit process env trust setting must win over the host trust DB fallback"
    );
}
