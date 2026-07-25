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

#[cfg(unix)]
#[test]
fn run_target_preflight_fails_closed_for_broken_external_symlink() {
    use std::os::unix::fs::symlink;

    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _cwd = CurrentDirGuard::enter(project.path());
    let lexical_target = project.path().join("target");
    let resolved_target = project.path().join("external-ssd").join("target");
    symlink(&resolved_target, &lexical_target).expect("create broken external target symlink");
    let mut env = HashMap::new();

    let error = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
    )
    .expect_err("broken canonical target must block the run before edits");

    assert!(error.contains("Cargo target preflight blocked before provider execution"));
    assert!(error.contains(&format!("lexical path '{}'", lexical_target.display())));
    assert!(error.contains(&format!("resolves to '{}'", resolved_target.display())));
    assert!(error.contains("will not substitute an alternate CARGO_TARGET_DIR"));
    assert!(
        !env.contains_key("CARGO_TARGET_DIR"),
        "preflight must not inject a managed alternate target"
    );
    assert!(
        std::fs::symlink_metadata(&lexical_target)
            .expect("target symlink metadata")
            .file_type()
            .is_symlink(),
        "preflight must preserve the configured canonical symlink"
    );
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
fn runtime_guard_replaces_readonly_ambient_target_for_unwritable_target() {
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
    assert_eq!(
        env.get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
            .map(String::as_str),
        Some("/usr/local"),
        "normal env merge must preserve the ambient target before the runtime guard decides"
    );

    let error = crate::pipeline_cargo_target::apply_runtime_task_target_dir_guards(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
        None,
    )
    .expect_err("unwritable canonical target must fail closed");

    assert!(error.contains("Cargo target preflight blocked before provider execution"));
    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/usr/local"),
        "preflight must not replace the configured target with a managed fallback"
    );
}

#[cfg(unix)]
#[test]
fn runtime_guard_replaces_readonly_caller_target_for_unwritable_target() {
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
    assert_eq!(
        env.get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
            .map(String::as_str),
        Some("/usr/local"),
        "normal env merge must preserve the caller target before the runtime guard decides"
    );

    let error = crate::pipeline_cargo_target::apply_runtime_task_target_dir_guards(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
        Some(&caller_env),
    )
    .expect_err("unwritable canonical target must fail closed");

    assert!(error.contains("Cargo target preflight blocked before provider execution"));
    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/usr/local"),
        "preflight must not replace the configured target with a managed fallback"
    );
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
    assert_eq!(
        env.get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
            .map(String::as_str),
        Some(explicit_target.as_str()),
        "normal env merge must preserve caller CARGO_TARGET_DIR"
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

    let error = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
    )
    .expect_err("unwritable canonical target must fail closed");

    assert!(error.contains("workspace_target_unwritable"));
    assert!(error.contains("will not substitute an alternate CARGO_TARGET_DIR"));
    assert!(
        !env.contains_key("CARGO_TARGET_DIR"),
        "preflight must not select a managed target"
    );
    assert!(
        !project.path().join("target/.cargo-build-lock").exists(),
        "preflight must not invoke Cargo"
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
