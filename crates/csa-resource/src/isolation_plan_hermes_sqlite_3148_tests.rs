//! SQLite generation and migration regressions for #3148.

use super::*;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
fn live_sqlite_database(path: &Path, value: &str) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA wal_autocheckpoint=0;
             CREATE TABLE values_table (value TEXT NOT NULL);
             INSERT INTO values_table (value) VALUES ('test');",
        )
        .unwrap();
    connection
        .execute("INSERT INTO values_table (value) VALUES (?1)", [value])
        .unwrap();
    connection
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

struct AtomicCopyFailure;

impl AtomicCopyFailure {
    fn set() -> Self {
        super::super::super::readable::FAIL_ATOMIC_COPY.with(|failure| failure.set(true));
        Self
    }
}

impl Drop for AtomicCopyFailure {
    fn drop(&mut self) {
        super::super::super::readable::FAIL_ATOMIC_COPY.with(|failure| failure.set(false));
    }
}

#[cfg(unix)]
#[test]
fn sqlite_migration_publishes_one_standalone_backup() {
    let temp = tempfile::Builder::new()
        .prefix("hermes-standalone-migration-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let source = temp.path().join("legacy");
    let destination = temp.path().join("runtime");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    let source_connection = live_sqlite_database(&source.join("state.db"), "source");
    assert!(
        source.join("state.db-wal").exists(),
        "source must have a live WAL"
    );

    super::super::super::hermes_paths::migrate_sqlite_generation(
        &std::fs::File::open(&source).unwrap(),
        &std::fs::File::open(&destination).unwrap(),
        std::ffi::OsStr::new("state.db"),
        std::fs::File::open(source.join("state.db")).unwrap(),
        &destination.join("state.db"),
    )
    .unwrap();
    drop(source_connection);

    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(
            !destination.join(format!("state.db{suffix}")).exists(),
            "standalone backup must not publish copied SQLite sidecar {suffix}"
        );
    }
    let migrated = rusqlite::Connection::open(destination.join("state.db")).unwrap();
    assert_eq!(
        migrated
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        migrated
            .query_row(
                "SELECT value FROM values_table ORDER BY rowid DESC LIMIT 1",
                [],
                |row| { row.get::<_, String>(0) }
            )
            .unwrap(),
        "source"
    );
}

#[cfg(unix)]
#[test]
fn sqlite_migration_is_owned_across_processes() {
    if let (Some(root), Some(value)) = (
        std::env::var_os("CSA_SQLITE_CHILD_ROOT"),
        std::env::var_os("CSA_SQLITE_CHILD_VALUE"),
    ) {
        let root = PathBuf::from(root);
        let source = root.join("source");
        let destination = PathBuf::from(std::env::var_os("CSA_SQLITE_CHILD_DESTINATION").unwrap());
        std::fs::create_dir_all(&source).unwrap();
        let connection = live_sqlite_database(&source.join("state.db"), &value.to_string_lossy());
        super::super::super::hermes_paths::migrate_sqlite_generation(
            &std::fs::File::open(&source).unwrap(),
            &std::fs::File::open(&destination).unwrap(),
            std::ffi::OsStr::new("state.db"),
            std::fs::File::open(source.join("state.db")).unwrap(),
            &destination.join("state.db"),
        )
        .unwrap();
        drop(connection);
        return;
    }

    let temp = tempfile::Builder::new()
        .prefix("hermes-cross-process-migration-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let destination = temp.path().join("runtime");
    let barrier = temp.path().join("barrier");
    std::fs::create_dir(&destination).unwrap();
    std::fs::create_dir(&barrier).unwrap();
    let test_name = "isolation_plan::tests::hermes_review_3148_tests::sqlite_3148_tests::sqlite_migration_is_owned_across_processes";
    let mut children = Vec::new();
    for (index, value) in ["first", "second"].into_iter().enumerate() {
        let root = temp.path().join(format!("source-{index}"));
        std::fs::create_dir(&root).unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg(&test_name)
            .arg("--nocapture")
            .env("CSA_SQLITE_CHILD_ROOT", &root)
            .env("CSA_SQLITE_CHILD_VALUE", value)
            .env("CSA_SQLITE_CHILD_DESTINATION", &destination)
            .env("CSA_SQLITE_PUBLICATION_BARRIER", &barrier);
        children.push(command.spawn().unwrap());
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let entered = std::fs::read_dir(&barrier).unwrap().count();
        if entered >= 2 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "both migration processes must reach the race barrier"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    std::fs::File::create(barrier.join("release")).unwrap();
    for mut child in children {
        assert!(child.wait().unwrap().success(), "migration child failed");
    }

    let migrated = rusqlite::Connection::open(destination.join("state.db")).unwrap();
    assert_eq!(
        migrated
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    let value: String = migrated
        .query_row(
            "SELECT value FROM values_table ORDER BY rowid DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(matches!(value.as_str(), "first" | "second"));
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
    let direct_connection = live_sqlite_database(&hermes_home.join("direct/state.db"), "direct");
    let nested_connection =
        live_sqlite_database(&hermes_home.join("profiles/nested/state.db"), "nested");
    let flat_connection = live_sqlite_database(&hermes_home.join("state.flat.db"), "flat");
    let layouts = [
        ("direct", hermes_home.join("direct/state.db"), "direct"),
        (
            "nested",
            hermes_home.join("profiles/nested/state.db"),
            "nested",
        ),
        ("flat", hermes_home.join("state.flat.db"), "flat"),
    ];

    hermes_plan(&hermes_home).expect("Hermes plan must migrate legacy profiles");
    drop((direct_connection, nested_connection, flat_connection));

    let runtime = hermes_home.join(".csa-runtime");
    for (profile, legacy, value) in &layouts {
        let resolved = crate::isolation_plan::resolve_hermes_state_db(&hermes_home, Some(profile));
        let runtime_db = match *profile {
            "direct" => runtime.join("direct/state.db"),
            "nested" => runtime.join("profiles/nested/state.db"),
            "flat" => runtime.join("state.flat.db"),
            _ => unreachable!(),
        };
        assert_eq!(resolved, runtime_db, "{profile} must resolve to runtime DB");
        let migrated = rusqlite::Connection::open(&runtime_db).unwrap();
        assert_eq!(
            migrated
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            migrated
                .query_row(
                    "SELECT value FROM values_table ORDER BY rowid DESC LIMIT 1",
                    [],
                    |row| { row.get::<_, String>(0) }
                )
                .unwrap(),
            *value
        );
        assert!(legacy.exists(), "legacy {profile} DB must remain in place");
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
    let direct_connection = live_sqlite_database(&hermes_home.join("direct/state.db"), "direct");
    let nested_connection =
        live_sqlite_database(&hermes_home.join("profiles/nested/state.db"), "nested");
    let flat_connection = live_sqlite_database(&hermes_home.join("state.flat.db"), "flat");

    let plan = hermes_plan(&hermes_home).expect("Hermes plan must migrate profile databases");
    drop((direct_connection, nested_connection, flat_connection));
    let args = command_args(&plan);
    let runtime = hermes_home.join(".csa-runtime");
    let layouts = [
        (
            "direct",
            hermes_home.join("direct/state.db"),
            runtime.join("direct/state.db"),
            "direct",
        ),
        (
            "nested",
            hermes_home.join("profiles/nested/state.db"),
            runtime.join("profiles/nested/state.db"),
            "nested",
        ),
        (
            "flat",
            hermes_home.join("state.flat.db"),
            runtime.join("state.flat.db"),
            "flat",
        ),
    ];

    for (profile, legacy_db, runtime_db, value) in &layouts {
        assert_eq!(
            crate::isolation_plan::resolve_hermes_state_db(&hermes_home, Some(profile)),
            *runtime_db,
            "{profile} must resolve to the migrated DB used by xurl and recall"
        );
        let migrated = rusqlite::Connection::open(runtime_db).unwrap();
        assert_eq!(
            migrated
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            migrated
                .query_row(
                    "SELECT value FROM values_table ORDER BY rowid DESC LIMIT 1",
                    [],
                    |row| { row.get::<_, String>(0) }
                )
                .unwrap(),
            *value
        );
        assert!(
            legacy_db.exists(),
            "legacy {profile} DB must remain in place"
        );

        let requested_dir = legacy_db.parent().unwrap();
        let writable = plan
            .readable_paths
            .iter()
            .find(|path| path.writable_bind() && path.requested() == requested_dir)
            .expect("migrated profile must have a pinned writable bind");
        assert_eq!(
            writable.bind_source(),
            runtime_db.parent().unwrap(),
            "{profile} writes must bind the migrated runtime directory"
        );
        let writable_pos = args
            .windows(3)
            .position(|window| {
                window[0] == "--bind-fd" && window[2] == requested_dir.to_string_lossy().as_ref()
            })
            .expect("migrated profile must have a pinned writable bind");
        if requested_dir == hermes_home {
            assert_eq!(profile, &"flat");
            assert_eq!(writable.bind_source(), &runtime);
            assert!(
                args.windows(3).any(|window| {
                    window[0] == "--bind-fd" && window[2] == hermes_home.to_string_lossy().as_ref()
                }),
                "flat profile must be covered by the pinned writable Hermes home bind: {args:?}"
            );
            continue;
        }
        let overlay = plan
            .readable_paths
            .iter()
            .filter(|path| {
                path.overrides_writable_mount() && requested_dir.starts_with(path.requested())
            })
            .max_by_key(|path| path.requested().components().count())
            .expect("migrated profile must remain under a read-only overlay");
        let overlay_pos = args
            .windows(3)
            .position(|window| {
                window[0] == "--ro-bind-fd"
                    && window[2] == overlay.requested().to_string_lossy().as_ref()
            })
            .expect("profile overlay must use its pinned read-only bind");
        assert!(
            overlay_pos < writable_pos,
            "writable {profile} bind must follow its parent overlay: {args:?}"
        );
    }
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
    let source_connection = live_sqlite_database(&hermes_home.join("state.db"), "legacy");

    let _failure = AtomicCopyFailure::set();
    let error = hermes_plan(&hermes_home).expect_err("injected copy failure must fail closed");
    drop(source_connection);
    assert!(error.to_string().contains("not writable"));
    assert!(!hermes_home.join(".csa-runtime/state.db").exists());
    assert!(!hermes_home.join(".csa-runtime/state.db-wal").exists());
    let legacy = rusqlite::Connection::open(hermes_home.join("state.db")).unwrap();
    assert_eq!(
        legacy
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
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
    let source_connection = live_sqlite_database(&source.join("state.db"), "threaded");

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
            super::super::super::hermes_paths::migrate_sqlite_generation(
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
    drop(source_connection);
    let migrated = rusqlite::Connection::open(destination.join("state.db")).unwrap();
    assert_eq!(
        migrated
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        migrated
            .query_row(
                "SELECT value FROM values_table ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "threaded"
    );
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
    let source_connection = live_sqlite_database(&hermes_home.join("state.db"), "legacy");
    assert!(hermes_home.join("state.db-wal").exists());
    let runtime = hermes_home.join(".csa-runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::write(runtime.join("state.db-wal"), b"stale-wal").unwrap();
    std::fs::write(runtime.join("state.db-shm"), b"stale-shm").unwrap();
    std::fs::write(runtime.join("state.db-journal"), b"stale-journal").unwrap();

    let _replacement = AfterHermesHomePinned::set(replace_pinned_home_with_injected_directory);
    hermes_plan(&hermes_home).expect("pinned legacy generation must survive pathname replacement");
    drop(source_connection);
    let relocated = temp.path().join("hermes-home-original/.csa-runtime");
    let migrated = rusqlite::Connection::open(relocated.join("state.db")).unwrap();
    assert_eq!(
        migrated
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        migrated
            .query_row(
                "SELECT value FROM values_table ORDER BY rowid DESC LIMIT 1",
                [],
                |row| { row.get::<_, String>(0) }
            )
            .unwrap(),
        "legacy"
    );
    assert_eq!(
        std::fs::read(temp.path().join("hermes-home-original/state.db-wal")).is_ok(),
        true
    );
    assert_eq!(
        std::fs::read(temp.path().join("injected-home/state.db")).is_err(),
        true
    );
}
