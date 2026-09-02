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

struct AtomicCopyFailure;

impl AtomicCopyFailure {
    fn set() -> Self {
        super::super::readable::FAIL_ATOMIC_COPY.with(|failure| failure.set(true));
        Self
    }
}

impl Drop for AtomicCopyFailure {
    fn drop(&mut self) {
        super::super::readable::FAIL_ATOMIC_COPY.with(|failure| failure.set(false));
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

#[cfg(unix)]
#[test]
fn legacy_profile_state_databases_migrate_to_runtime_for_all_supported_layouts() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-profile-migration-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    std::fs::create_dir_all(hermes_home.join("profiles").join("nested")).unwrap();
    std::fs::create_dir_all(hermes_home.join("direct")).unwrap();
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
    let layouts = [
        (
            "direct",
            hermes_home.join("direct/state.db"),
            b"direct-db".as_slice(),
        ),
        (
            "nested",
            hermes_home.join("profiles/nested/state.db"),
            b"nested-db".as_slice(),
        ),
        (
            "flat",
            hermes_home.join("state.flat.db"),
            b"flat-db".as_slice(),
        ),
    ];
    for (_, path, contents) in &layouts {
        std::fs::write(path, contents).unwrap();
    }

    hermes_plan(&hermes_home).expect("Hermes plan must migrate legacy profiles");

    let runtime = hermes_home.join(".csa-runtime");
    for (profile, legacy, contents) in &layouts {
        let resolved = crate::isolation_plan::resolve_hermes_state_db(&hermes_home, Some(profile));
        let runtime_db = match *profile {
            "direct" => runtime.join("direct/state.db"),
            "nested" => runtime.join("profiles/nested/state.db"),
            "flat" => runtime.join("state.flat.db"),
            _ => unreachable!(),
        };
        assert_eq!(resolved, runtime_db, "{profile} must resolve to runtime DB");
        assert_eq!(std::fs::read(&runtime_db).unwrap(), *contents);
        assert_eq!(std::fs::read(legacy).unwrap(), *contents);
    }
}

#[cfg(unix)]
#[test]
fn migrated_profile_databases_remain_writable_through_bwrap_overlays() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-profile-bwrap-write-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("direct")).unwrap();
    std::fs::create_dir_all(hermes_home.join("profiles/nested")).unwrap();
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
    std::fs::write(hermes_home.join("direct/state.db"), b"direct-db\n").unwrap();
    std::fs::write(hermes_home.join("profiles/nested/state.db"), b"nested-db\n").unwrap();
    std::fs::write(hermes_home.join("state.flat.db"), b"flat-db\n").unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir(&project).unwrap();

    let execution_env = std::collections::HashMap::from([(
        "HERMES_HOME".to_string(),
        hermes_home.to_string_lossy().into_owned(),
    )]);
    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_execution_env(Some(&execution_env))
        .with_tool_defaults("hermes", &project, Path::new("/tmp/session"))
        .build()
        .expect("Hermes plan must migrate profile databases");
    let mut command = crate::from_isolation_plan(
        &plan,
        "/bin/sh",
        &[
            "-c".to_string(),
            format!(
                "printf 'write\\n' >> '{}/direct/state.db'; printf 'write\\n' >> '{}/profiles/nested/state.db'; printf 'write\\n' >> '{}/state.flat.db'",
                hermes_home.display(),
                hermes_home.display(),
                hermes_home.display(),
            ),
        ],
    )
    .expect("Hermes plan must produce a bwrap command");
    let status = command.status().expect("bwrap command must start");
    assert!(
        status.success(),
        "bwrap profile write must succeed: {status}"
    );

    let runtime = hermes_home.join(".csa-runtime");
    for (profile, path) in [
        ("direct", runtime.join("direct/state.db")),
        ("nested", runtime.join("profiles/nested/state.db")),
        ("flat", runtime.join("state.flat.db")),
    ] {
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            contents.ends_with("write\n"),
            "bwrap write for {profile} must reach runtime DB {path:?}: {contents:?}"
        );
    }
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

#[cfg(unix)]
#[test]
fn sqlite_migration_is_fd_pinned_and_atomic_on_copy_failure() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-atomic-migration-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    std::fs::create_dir_all(hermes_home.join("profiles").join("nested")).unwrap();
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
    std::fs::write(hermes_home.join("state.db"), b"legacy-db").unwrap();
    std::fs::write(hermes_home.join("state.db-wal"), b"legacy-wal").unwrap();
    std::fs::write(hermes_home.join("state.db-shm"), b"legacy-shm").unwrap();
    std::fs::write(hermes_home.join("state.db-journal"), b"legacy-journal").unwrap();

    let _failure = AtomicCopyFailure::set();
    let error = hermes_plan(&hermes_home).expect_err("injected copy failure must fail closed");
    assert!(error.to_string().contains("not writable"));
    assert!(!hermes_home.join(".csa-runtime/state.db").exists());
    assert!(!hermes_home.join(".csa-runtime/state.db-wal").exists());
    assert_eq!(
        std::fs::read(hermes_home.join("state.db")).unwrap(),
        b"legacy-db"
    );
}

#[cfg(unix)]
#[test]
fn concurrent_sqlite_migrations_preserve_one_complete_generation() {
    let temp = tempfile::Builder::new()
        .prefix("hermes-concurrent-migration-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let source = temp.path().join("legacy");
    let destination = temp.path().join("runtime");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    for (suffix, contents) in [
        ("", b"database".as_slice()),
        ("-wal", b"wal".as_slice()),
        ("-shm", b"shm".as_slice()),
        ("-journal", b"journal".as_slice()),
    ] {
        std::fs::write(source.join(format!("state.db{suffix}")), contents).unwrap();
    }

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let source_parent = std::fs::File::open(&source).unwrap();
        let destination_parent = std::fs::File::open(&destination).unwrap();
        let source_database = std::fs::File::open(source.join("state.db")).unwrap();
        let destination_path = destination.join("state.db");
        let barrier = std::sync::Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            super::super::hermes_paths::migrate_sqlite_generation(
                &source_parent,
                &destination_parent,
                std::ffi::OsStr::new("state.db"),
                source_database,
                &destination_path,
            )
        }));
    }
    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    for (suffix, contents) in [
        ("", b"database".as_slice()),
        ("-wal", b"wal".as_slice()),
        ("-shm", b"shm".as_slice()),
        ("-journal", b"journal".as_slice()),
    ] {
        assert_eq!(
            std::fs::read(destination.join(format!("state.db{suffix}"))).unwrap(),
            contents,
            "published SQLite generation must remain complete"
        );
    }
}

#[cfg(unix)]
#[test]
fn sqlite_migration_keeps_wal_generation_coherent_after_path_replacement() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-coherent-migration-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
    for (suffix, contents) in [
        ("", b"legacy-db".as_slice()),
        ("-wal", b"legacy-wal".as_slice()),
        ("-shm", b"legacy-shm".as_slice()),
        ("-journal", b"legacy-journal".as_slice()),
    ] {
        std::fs::write(hermes_home.join(format!("state.db{suffix}")), contents).unwrap();
    }
    let runtime = hermes_home.join(".csa-runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::write(runtime.join("state.db-wal"), b"stale-wal").unwrap();
    std::fs::write(runtime.join("state.db-shm"), b"stale-shm").unwrap();
    std::fs::write(runtime.join("state.db-journal"), b"stale-journal").unwrap();

    let _replacement = AfterHermesHomePinned::set(replace_pinned_home_with_injected_directory);
    hermes_plan(&hermes_home).expect("pinned legacy generation must survive pathname replacement");
    let relocated = temp.path().join("hermes-home-original/.csa-runtime");
    for (suffix, contents) in [
        ("", b"legacy-db".as_slice()),
        ("-wal", b"legacy-wal".as_slice()),
        ("-shm", b"legacy-shm".as_slice()),
        ("-journal", b"legacy-journal".as_slice()),
    ] {
        assert_eq!(
            std::fs::read(relocated.join(format!("state.db{suffix}"))).unwrap(),
            contents
        );
    }
    assert_eq!(
        std::fs::read(temp.path().join("hermes-home-original/state.db")).unwrap(),
        b"legacy-db"
    );
}
