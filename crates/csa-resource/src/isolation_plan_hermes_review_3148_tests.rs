//! Canonical #3148 successor regressions after `01M1GEQA2FM2PK00ZPRXTK0V3R`.

use super::*;
use std::path::Path;

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

fn command_args(plan: &IsolationPlan) -> Vec<String> {
    crate::from_isolation_plan(plan, "/usr/bin/tool", &[])
        .expect("planned Hermes sandbox must produce a bwrap command")
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn is_independent_bind(args: &[String], path: &Path) -> bool {
    let dest = path.to_string_lossy();
    args.windows(3).any(|window| {
        matches!(window[0].as_str(), "--bind" | "--bind-fd") && window[2] == dest.as_ref()
    })
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
fn sqlite_journal_is_not_an_independent_file_mountpoint() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-sqlite-mount-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
    std::fs::write(hermes_home.join("state.db"), b"").unwrap();

    let plan = hermes_plan(&hermes_home).expect("Hermes sqlite plan must build");
    let args = command_args(&plan);
    for name in ["state.db-wal", "state.db-shm", "state.db-journal"] {
        let sidecar = hermes_home.join(name);
        assert!(
            !plan.writable_paths.contains(&sidecar),
            "{name} must not be an independent writable mountpoint"
        );
        assert!(
            !is_independent_bind(&args, &sidecar),
            "{name} must not be a --bind/--bind-fd mountpoint; args: {args:?}"
        );
    }
    assert!(
        plan.writable_paths.contains(&hermes_home),
        "SQLite journal unlink needs one pinned writable Hermes home directory"
    );
    let writable_home = plan
        .readable_paths
        .iter()
        .find(|path| path.writable_bind() && path.requested() == hermes_home)
        .expect("Hermes home must use a pinned writable bind");
    assert_eq!(
        writable_home.bind_source(),
        hermes_home.join(".csa-runtime"),
        "Hermes home must bind a dedicated runtime backing, not the host home"
    );
}
