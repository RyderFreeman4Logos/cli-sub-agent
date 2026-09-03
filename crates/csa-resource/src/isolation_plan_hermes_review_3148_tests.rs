//! Canonical #3148 successor regressions after `01M1GEQA2FM2PK00ZPRXTK0V3R`.

use super::*;
use std::path::Path;

#[path = "isolation_plan_hermes_sqlite_3148_tests.rs"]
mod sqlite_3148_tests;

#[test]
fn empty_child_hermes_home_does_not_select_ambient_hermes_home() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let ambient = temp.path().join("ambient-hermes");
    std::fs::create_dir_all(ambient.join("logs")).unwrap();
    let _ambient = ScopedEnvVar::set("HERMES_HOME", &ambient);
    let execution_env =
        std::collections::HashMap::from([("HERMES_HOME".to_string(), String::new())]);

    let result = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_execution_env(Some(&execution_env))
        .with_tool_defaults(
            "hermes",
            Path::new("/tmp/project"),
            Path::new("/tmp/session"),
        )
        .build();

    match result {
        Err(error) => {
            assert!(
                error
                    .to_string()
                    .contains("hermes sandbox preflight failed"),
                "empty child HERMES_HOME must fail closed: {error:#}"
            );
        }
        Ok(plan) => {
            panic!(
                "empty child HERMES_HOME selected ambient path {:?}; writable={:?} readable={:?}",
                ambient,
                plan.writable_paths,
                plan.readable_paths
                    .iter()
                    .map(|path| path.requested().to_path_buf())
                    .collect::<Vec<_>>(),
            );
        }
    }
}

#[test]
fn empty_child_home_does_not_select_ambient_home() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (home, _env) = isolated_home(&temp);
    let _hermes_home_env = ScopedEnvVar::unset("HERMES_HOME");
    let execution_env = std::collections::HashMap::from([("HOME".to_string(), String::new())]);

    let result = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_execution_env(Some(&execution_env))
        .with_tool_defaults(
            "hermes",
            Path::new("/tmp/project"),
            Path::new("/tmp/session"),
        )
        .build();

    match result {
        Err(error) => {
            assert!(
                error
                    .to_string()
                    .contains("hermes sandbox preflight failed"),
                "empty child HOME must fail closed: {error:#}"
            );
        }
        Ok(plan) => {
            let ambient = home.join(".hermes");
            panic!(
                "empty child HOME selected ambient path {:?}; writable={:?} readable={:?}",
                ambient,
                plan.writable_paths,
                plan.readable_paths
                    .iter()
                    .map(|path| path.requested().to_path_buf())
                    .collect::<Vec<_>>(),
            );
        }
    }
}

struct AfterHermesHomePinned;

impl AfterHermesHomePinned {
    fn set(inject: fn(&Path)) -> Self {
        super::super::hermes_paths::AFTER_HERMES_HOME_PINNED.with(|hook| hook.set(Some(inject)));
        Self
    }
}

impl Drop for AfterHermesHomePinned {
    fn drop(&mut self) {
        super::super::hermes_paths::AFTER_HERMES_HOME_PINNED.with(|hook| hook.set(None));
    }
}

struct ReaddirErrorAfter;

impl ReaddirErrorAfter {
    fn set(entries: usize) -> Self {
        super::super::readable::READDIR_ERROR_AFTER.with(|after| after.set(Some(entries)));
        Self
    }
}

impl Drop for ReaddirErrorAfter {
    fn drop(&mut self) {
        super::super::readable::READDIR_ERROR_AFTER.with(|after| after.set(None));
    }
}

fn replace_pinned_home_with_injected_directory(hermes_home: &Path) {
    let parent = hermes_home.parent().expect("Hermes home parent");
    let relocated = parent.join("hermes-home-original");
    let injected = parent.join("injected-home");
    std::fs::create_dir_all(injected.join("logs")).expect("create injected logs");
    std::fs::write(injected.join("evil.yaml"), "injected\n").expect("write injected config");
    std::fs::rename(hermes_home, &relocated).expect("relocate pinned home");
    std::fs::rename(&injected, hermes_home).expect("install replacement home");
}

fn hermes_plan(hermes_home: &Path) -> anyhow::Result<IsolationPlan> {
    let execution_env = std::collections::HashMap::from([(
        "HERMES_HOME".to_string(),
        hermes_home.to_string_lossy().into_owned(),
    )]);
    IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_execution_env(Some(&execution_env))
        .with_tool_defaults(
            "hermes",
            Path::new("/tmp/project"),
            Path::new("/tmp/session"),
        )
        .build()
}

#[cfg(unix)]
#[test]
fn hermes_tmp_home_fails_preflight_without_panic_or_filesystem_side_effects() {
    let _guard = ENV_LOCK.lock().unwrap();
    let runtime = Path::new("/tmp/.csa-runtime");
    let runtime_existed = runtime.exists();
    let execution_env =
        std::collections::HashMap::from([("HERMES_HOME".to_string(), "/tmp".to_string())]);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .with_execution_env(Some(&execution_env))
            .with_tool_defaults(
                "hermes",
                Path::new("/tmp/project"),
                Path::new("/tmp/session"),
            )
            .build()
    }));

    let plan_result = result.expect("HERMES_HOME=/tmp must not panic");
    let error = plan_result.expect_err("HERMES_HOME=/tmp must fail preflight");
    assert!(
        error
            .to_string()
            .contains("hermes sandbox preflight failed"),
        "unsafe Hermes home must fail with a preflight error: {error:#}"
    );
    assert_eq!(
        runtime.exists(),
        runtime_existed,
        "rejecting HERMES_HOME=/tmp must not create or remove runtime state"
    );
}

#[cfg(unix)]
#[test]
fn hermes_config_enumeration_uses_pinned_home_fd_not_pathname() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-home-fd-enum-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    std::fs::write(hermes_home.join("config.yaml"), "pinned\n").unwrap();
    let _hook = AfterHermesHomePinned::set(replace_pinned_home_with_injected_directory);

    let result = hermes_plan(&hermes_home);
    match result {
        Err(error) => {
            assert!(
                error
                    .to_string()
                    .contains("hermes sandbox preflight failed"),
                "pathname replacement after home pin must fail closed: {error:#}"
            );
        }
        Ok(plan) => {
            assert!(
                !plan.readable_paths.iter().any(|path| {
                    path.requested().file_name() == Some(std::ffi::OsStr::new("evil.yaml"))
                }),
                "replacement directory config must not be enumerated from the pathname"
            );
            assert!(
                plan.readable_paths
                    .iter()
                    .any(|path| path.requested() == hermes_home.join("config.yaml").as_path()),
                "pinned home fd must retain the original config overlay"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn hermes_config_enumeration_fails_closed_on_midstream_readdir_error() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-home-readdir-error-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
    let _fault = ReaddirErrorAfter::set(2);

    let error = hermes_plan(&hermes_home)
        .expect_err("mid-stream readdir error must not produce a partial Hermes plan");
    assert!(
        error
            .to_string()
            .contains("cannot enumerate Hermes configuration overlays"),
        "readdir error must propagate through overlay_enumeration_error: {error:#}"
    );
}

#[cfg(unix)]
#[test]
fn tier4_hermes_rejects_readonly_existing_runtime_backing() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-runtime-write-probe-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    std::fs::create_dir(hermes_home.join("profiles")).unwrap();
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
    let runtime = hermes_home.join(".csa-runtime");
    std::fs::create_dir(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o555)).unwrap();

    let result = hermes_plan(&hermes_home);

    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = result.expect_err("read-only .csa-runtime must fail Hermes preflight");
    assert!(
        error.to_string().contains("not writable"),
        "runtime backing failure must identify writability: {error:#}"
    );
}
