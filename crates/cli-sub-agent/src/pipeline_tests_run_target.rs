use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tempfile::tempdir;

fn current_dir_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct CurrentDirGuard {
    original: PathBuf,
}

impl CurrentDirGuard {
    fn enter(path: &std::path::Path) -> Self {
        let original = std::env::current_dir().expect("read current dir");
        std::env::set_current_dir(path).expect("set current dir");
        Self { original }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.original).expect("restore current dir");
    }
}

#[test]
fn apply_run_target_dir_guard_leaves_existing_directory_target_untouched() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _cwd = CurrentDirGuard::enter(project.path());
    std::fs::create_dir(project.path().join("target")).expect("create target dir");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/tmp/codex-session-target".to_string(),
    );

    let report = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
    )
    .expect("policy should resolve");

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/tmp/codex-session-target")
    );
    assert!(report.explicit_override_preserved);
    assert!(!report.automatic_substitution_applied);
}

#[cfg(unix)]
#[test]
fn apply_run_target_dir_guard_preserves_explicit_env_with_broken_target_symlink() {
    use std::os::unix::fs::symlink;

    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _cwd = CurrentDirGuard::enter(project.path());
    symlink("missing-mount/target", project.path().join("target"))
        .expect("create broken target symlink");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/tmp/codex-session-target".to_string(),
    );

    let report = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
    )
    .expect("policy should resolve");

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/tmp/codex-session-target")
    );
    assert!(report.explicit_override_preserved);
    assert!(!report.automatic_substitution_applied);
}

#[test]
fn apply_run_target_dir_guard_does_not_inject_override_when_repo_target_missing() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _cwd = CurrentDirGuard::enter(project.path());
    let mut env = HashMap::new();

    let report = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
    )
    .expect("policy should resolve");

    assert!(
        !env.contains_key("CARGO_TARGET_DIR"),
        "run guard must not invent a CSA override when ./target is absent"
    );
    assert_eq!(report.policy_reason, "workspace_target_writable");
    assert_eq!(
        report.workspace_target_status,
        "workspace_target_absent_cargo_default"
    );
    assert!(!report.automatic_substitution_applied);
}

#[test]
fn apply_run_target_dir_guard_preserves_existing_env_when_repo_target_missing() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _cwd = CurrentDirGuard::enter(project.path());
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/tmp/codex-session-target".to_string(),
    );

    let report = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
    )
    .expect("policy should resolve");

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/tmp/codex-session-target")
    );
    assert!(report.explicit_override_preserved);
    assert!(!report.automatic_substitution_applied);
}

#[cfg(unix)]
#[test]
fn apply_run_target_dir_guard_preserves_absolute_workspace_target_override() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _state_home =
        crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", project.path().join("state"));
    let _cwd = CurrentDirGuard::enter(project.path());
    make_unwritable_target(project.path());
    let explicit_target = project.path().join("target").to_string_lossy().into_owned();
    let mut env = HashMap::from([("CARGO_TARGET_DIR".to_string(), explicit_target.clone())]);

    let report = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
    )
    .expect("policy should resolve");

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some(explicit_target.as_str())
    );
    assert_eq!(report.policy_reason, "explicit_override_preserved");
    assert_eq!(report.selected_cargo_target, explicit_target);
    assert!(report.explicit_override_preserved);
    assert!(!report.automatic_substitution_applied);
}

#[cfg(unix)]
#[test]
fn apply_run_target_dir_guard_preserves_relative_workspace_target_override() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _state_home =
        crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", project.path().join("state"));
    let _cwd = CurrentDirGuard::enter(project.path());
    make_unwritable_target(project.path());
    let mut env = HashMap::from([("CARGO_TARGET_DIR".to_string(), "target".to_string())]);

    let report = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
    )
    .expect("policy should resolve");

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("target")
    );
    assert_eq!(report.policy_reason, "explicit_override_preserved");
    assert_eq!(report.selected_cargo_target, "target");
    assert!(report.explicit_override_preserved);
    assert!(!report.automatic_substitution_applied);
}

#[cfg(unix)]
#[test]
fn runtime_guard_replaces_synthesized_workspace_target_for_unwritable_target() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let project = tempdir().expect("tempdir");
    let home = project.path().join("home");
    std::fs::create_dir_all(&home).expect("create home");
    let _home = crate::test_env_lock::ScopedEnvVarRestore::set("HOME", &home);
    let _state_home = crate::test_env_lock::ScopedEnvVarRestore::set(
        "XDG_STATE_HOME",
        project.path().join("state"),
    );
    let _cargo_home = crate::test_env_lock::ScopedEnvVarRestore::set(
        csa_core::env::CARGO_HOME_ENV_KEY,
        "/usr/local",
    );
    let _cargo_install_root = crate::test_env_lock::ScopedEnvVarRestore::set(
        csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY,
        "/usr/local",
    );
    let _cargo_target_dir = crate::test_env_lock::ScopedEnvVarRestore::set(
        csa_core::env::CARGO_TARGET_DIR_ENV_KEY,
        "/usr/local",
    );
    make_unwritable_target(project.path());
    let mut env = crate::pipeline_env::build_merged_env(crate::pipeline_env::MergedEnvRequest {
        extra_env: None,
        config: None,
        global_config: None,
        project_root: Some(project.path()),
        tool_name: "codex",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });
    let synthesized_target = project.path().join("target").to_string_lossy().into_owned();
    assert_eq!(
        env.get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
            .map(String::as_str),
        Some(synthesized_target.as_str())
    );

    let report = crate::pipeline_cargo_target::apply_runtime_task_target_dir_guards(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
        None,
    )
    .expect("policy should resolve");

    let selected = PathBuf::from(env.get("CARGO_TARGET_DIR").expect("managed target env"));
    assert!(selected.ends_with("cargo-target"));
    assert_ne!(selected, project.path().join("target"));
    assert_eq!(report.policy_reason, "managed_target_selected");
    assert!(!report.explicit_override_preserved);
    assert!(report.automatic_substitution_applied);
}

#[cfg(unix)]
#[test]
fn runtime_guard_replaces_config_normalized_workspace_target_for_unwritable_target() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let project = tempdir().expect("tempdir");
    let home = project.path().join("home");
    std::fs::create_dir_all(&home).expect("create home");
    let _home = crate::test_env_lock::ScopedEnvVarRestore::set("HOME", &home);
    let _state_home = crate::test_env_lock::ScopedEnvVarRestore::set(
        "XDG_STATE_HOME",
        project.path().join("state"),
    );
    let _cargo_target_dir =
        crate::test_env_lock::ScopedEnvVarRestore::unset(csa_core::env::CARGO_TARGET_DIR_ENV_KEY);
    make_unwritable_target(project.path());
    let caller_env = HashMap::from([(
        csa_core::env::CARGO_TARGET_DIR_ENV_KEY.to_string(),
        "/usr/local".to_string(),
    )]);
    let mut env = crate::pipeline_env::build_merged_env(crate::pipeline_env::MergedEnvRequest {
        extra_env: Some(&caller_env),
        config: None,
        global_config: None,
        project_root: Some(project.path()),
        tool_name: "codex",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });
    let synthesized_target = project.path().join("target").to_string_lossy().into_owned();
    assert_eq!(
        env.get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
            .map(String::as_str),
        Some(synthesized_target.as_str())
    );

    let report = crate::pipeline_cargo_target::apply_runtime_task_target_dir_guards(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
        Some(&caller_env),
    )
    .expect("policy should resolve");

    let selected = PathBuf::from(env.get("CARGO_TARGET_DIR").expect("managed target env"));
    assert!(selected.ends_with("cargo-target"));
    assert_ne!(selected, project.path().join("target"));
    assert_eq!(report.policy_reason, "managed_target_selected");
    assert!(!report.explicit_override_preserved);
    assert!(report.automatic_substitution_applied);
}

#[cfg(unix)]
#[test]
fn runtime_guard_preserves_external_caller_supplied_target_override() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let project = tempdir().expect("tempdir");
    let home = project.path().join("home");
    let explicit_target_path = project.path().join("explicit-cargo-target");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&explicit_target_path).expect("create explicit target");
    let _home = crate::test_env_lock::ScopedEnvVarRestore::set("HOME", &home);
    let _state_home = crate::test_env_lock::ScopedEnvVarRestore::set(
        "XDG_STATE_HOME",
        project.path().join("state"),
    );
    let _cargo_target_dir =
        crate::test_env_lock::ScopedEnvVarRestore::unset(csa_core::env::CARGO_TARGET_DIR_ENV_KEY);
    make_unwritable_target(project.path());
    let explicit_target = explicit_target_path.to_string_lossy().into_owned();
    let caller_env = HashMap::from([(
        csa_core::env::CARGO_TARGET_DIR_ENV_KEY.to_string(),
        explicit_target.clone(),
    )]);
    let mut env = crate::pipeline_env::build_merged_env(crate::pipeline_env::MergedEnvRequest {
        extra_env: Some(&caller_env),
        config: None,
        global_config: None,
        project_root: Some(project.path()),
        tool_name: "codex",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });
    let synthesized_target = project.path().join("target").to_string_lossy().into_owned();
    assert_eq!(
        env.get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
            .map(String::as_str),
        Some(synthesized_target.as_str()),
        "env merge pins Cargo target before the runtime guard restores caller intent"
    );

    let report = crate::pipeline_cargo_target::apply_runtime_task_target_dir_guards(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
        Some(&caller_env),
    )
    .expect("policy should resolve");

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some(explicit_target.as_str())
    );
    assert_eq!(report.policy_reason, "explicit_override_preserved");
    assert_eq!(report.selected_cargo_target, explicit_target);
    assert!(report.explicit_override_preserved);
    assert!(!report.automatic_substitution_applied);
}

#[cfg(unix)]
#[test]
fn cargo_target_detects_unwritable_workspace_target() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _state_home =
        crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", project.path().join("state"));
    let _cwd = CurrentDirGuard::enter(project.path());
    make_unwritable_target(project.path());
    let mut env = HashMap::new();

    let report = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
    )
    .expect("policy should resolve");

    let selected = PathBuf::from(env.get("CARGO_TARGET_DIR").expect("managed target env"));
    assert!(selected.ends_with("cargo-target"));
    assert!(selected.is_dir());
    assert_eq!(report.policy_reason, "managed_target_selected");
    assert_eq!(
        report.workspace_target_status,
        "workspace_target_unwritable"
    );
    assert!(report.automatic_substitution_applied);
    assert!(!project.path().join("target/.cargo-build-lock").exists());
}

#[cfg(unix)]
#[test]
fn cargo_target_policy_is_recorded_in_session_artifacts() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _state_home =
        crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", project.path().join("state"));
    let session_dir = project.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("create session dir");
    make_unwritable_target(project.path());
    let mut env = HashMap::new();
    let report = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
    )
    .expect("policy should resolve");

    crate::pipeline_cargo_target::persist_cargo_target_policy_artifact(&session_dir, &report)
        .expect("write policy artifact");

    let artifact = session_dir.join(crate::pipeline_cargo_target::CARGO_TARGET_POLICY_ARTIFACT);
    let raw = std::fs::read_to_string(&artifact).expect("read policy artifact");
    let parsed: toml::Value = toml::from_str(&raw).expect("parse policy artifact");
    assert_eq!(
        parsed["policy_reason"].as_str(),
        Some("managed_target_selected")
    );
    assert_eq!(
        parsed["original_workspace_target"].as_str(),
        Some(
            project
                .path()
                .join("target")
                .to_str()
                .expect("target path utf8")
        )
    );
    assert_eq!(
        parsed["selected_cargo_target"].as_str(),
        env.get("CARGO_TARGET_DIR").map(String::as_str)
    );
    assert_eq!(parsed["explicit_override_preserved"].as_bool(), Some(false));
    assert_eq!(
        parsed["automatic_substitution_applied"].as_bool(),
        Some(true)
    );
}

#[cfg(unix)]
#[test]
fn cargo_target_writeability_regression() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _state_home =
        crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", project.path().join("state"));
    let _cwd = CurrentDirGuard::enter(project.path());
    std::fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname = \"cargo-target-regression\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    )
    .expect("write Cargo.toml");
    std::fs::create_dir(project.path().join("src")).expect("create src dir");
    std::fs::write(
        project.path().join("src/lib.rs"),
        "pub fn value() -> u8 { 1 }\n",
    )
    .expect("write lib.rs");
    make_unwritable_target(project.path());
    let mut env = HashMap::new();

    crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
    )
    .expect("policy should resolve");
    let selected = env
        .get("CARGO_TARGET_DIR")
        .expect("managed CARGO_TARGET_DIR should be set");

    let output = std::process::Command::new("cargo")
        .arg("check")
        .env("CARGO_TARGET_DIR", selected)
        .current_dir(project.path())
        .output()
        .expect("run cargo check");

    assert!(
        output.status.success(),
        "cargo check should use managed target\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !project.path().join("target/.cargo-build-lock").exists(),
        "Cargo must not lock the unwritable workspace target"
    );
    assert!(
        Path::new(selected).join("debug/.cargo-build-lock").exists()
            || Path::new(selected).join(".rustc_info.json").exists(),
        "managed target should receive Cargo artifacts"
    );
}

#[cfg(unix)]
fn make_unwritable_target(project_root: &Path) {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let target = project_root.join("target");
    let proc_dir = Path::new("/proc");
    if proc_dir.is_dir() {
        symlink(proc_dir, &target).expect("create /proc target symlink");
        return;
    }

    std::fs::create_dir(&target).expect("create target dir");
    let mut permissions = std::fs::metadata(&target)
        .expect("target metadata")
        .permissions();
    permissions.set_mode(0o555);
    std::fs::set_permissions(&target, permissions).expect("make target read-only");
}
