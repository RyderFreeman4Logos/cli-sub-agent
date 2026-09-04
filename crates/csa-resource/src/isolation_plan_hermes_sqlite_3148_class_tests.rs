//! Class-sweep regressions for CSA-R3148-005/006/008/009/010/011.
//!
//! These tests cover invalid SQLite generations, pathname sidecar unlink,
//! bounded backup, and unnamed snapshot lifetime.

use super::*;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const LAYOUTS: [(&str, &str, &str); 4] = [
    ("root", "runtime", "state.db"),
    ("flat", "runtime", "state.flat.db"),
    ("direct", "runtime/direct", "state.db"),
    ("nested", "runtime/profiles/nested", "state.db"),
];

const SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

fn migrate_generation(source: &Path, destination: &Path, base: &str) -> anyhow::Result<()> {
    super::super::super::super::hermes_paths::migrate_sqlite_generation(
        &std::fs::File::open(source).unwrap(),
        &std::fs::File::open(destination).unwrap(),
        &std::fs::File::open(source).unwrap(),
        std::ffi::OsStr::new(base),
        std::fs::File::open(source.join(base)).unwrap(),
        &destination.join(base),
    )
}

fn layout_dirs(label: &str, dest_rel: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::Builder::new()
        .prefix(&format!("hermes-class-{label}-"))
        .tempdir_in("/var/tmp")
        .unwrap();
    let source = temp.path().join("legacy");
    let destination = temp.path().join(dest_rel);
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir_all(&destination).unwrap();
    (temp, source, destination)
}

fn staging_snapshot_names(root: &Path) -> Vec<PathBuf> {
    let mut names = Vec::new();
    let staging = root.join(".csa-sqlite-staging");
    let mut dirs = vec![root.to_path_buf()];
    if staging.is_dir() {
        dirs.push(staging);
    }
    for dir in dirs {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy();
            if name.starts_with(".csa-sqlite-snapshot-") {
                names.push(path);
            }
        }
    }
    names
}

fn corrupt_sqlite_btree(path: &Path) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch(
            "PRAGMA page_size=4096;
             CREATE TABLE corrupt_table (value TEXT NOT NULL);
             INSERT INTO corrupt_table (value) VALUES ('corrupt');",
        )
        .unwrap();
    drop(connection);
    assert_eq!(std::fs::metadata(path).unwrap().len(), 8192);
    let mut database = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    database.seek(SeekFrom::Start(4096)).unwrap();
    database.write_all(&[0]).unwrap();
    database.sync_all().unwrap();
    let connection = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA page_count", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2,
        "fixture must pass the old page-count-only check"
    );
}

fn make_snapshot_directory_read_only(snapshot_path: &Path) {
    let target = std::fs::read_link(snapshot_path).unwrap();
    std::fs::set_permissions(
        target.parent().unwrap(),
        std::fs::Permissions::from_mode(0o500),
    )
    .unwrap();
}

fn mark_snapshot_source_opened(source_path: &Path) {
    let source = std::fs::read_link(source_path).unwrap();
    std::fs::write(source.parent().unwrap().join("backup-entered"), []).unwrap();
    if let Some(root) = std::env::var_os("CSA_SQLITE_SNAPSHOT_DEATH_ROOT") {
        while !PathBuf::from(&root).join("release").exists() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(unix)]
#[test]
fn sqlite_migration_rejects_magic_only_sqlite_header_for_all_layouts() {
    for (label, dest_rel, base) in LAYOUTS {
        let (_temp, source, destination) = layout_dirs(&format!("magic-{label}"), dest_rel);
        drop(live_sqlite_database(&source.join(base), label));
        std::fs::write(destination.join(base), SQLITE_MAGIC).unwrap();

        let error = migrate_generation(&source, &destination, base).expect_err(label);
        assert!(
            error.to_string().contains("SQLite") || error.to_string().contains("generation"),
            "{label} must reject a magic-only header: {error:#}"
        );
        assert_eq!(
            std::fs::read(destination.join(base)).unwrap(),
            SQLITE_MAGIC,
            "{label} must preserve the truncated runtime DB"
        );
    }
}

#[cfg(unix)]
#[test]
fn sqlite_migration_preserves_valid_legacy_sqlite_for_all_layouts() {
    for (label, dest_rel, base) in LAYOUTS {
        let (_temp, source, destination) = layout_dirs(&format!("valid-{label}"), dest_rel);
        drop(live_sqlite_database(&source.join(base), "source"));
        drop(live_sqlite_database(&destination.join(base), label));

        migrate_generation(&source, &destination, base).expect(label);

        let preserved = rusqlite::Connection::open(destination.join(base)).unwrap();
        assert_eq!(
            preserved
                .query_row(
                    "SELECT value FROM values_table ORDER BY rowid DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            label,
            "{label} must keep a usable legacy generation"
        );
    }
}

#[cfg(unix)]
#[test]
fn sqlite_snapshot_rejects_oversized_source_before_parent_memory_clone_for_all_layouts() {
    for (label, dest_rel, base) in LAYOUTS {
        let (_temp, source, destination) = layout_dirs(&format!("oversized-{label}"), dest_rel);
        drop(live_sqlite_database(&source.join(base), label));
        std::fs::OpenOptions::new()
            .write(true)
            .open(source.join(base))
            .unwrap()
            .set_len(3 * 1024 * 1024)
            .unwrap();

        let error = migrate_generation(&source, &destination, base).expect_err(label);
        assert!(
            error.to_string().contains("SQLite") || error.to_string().contains("snapshot"),
            "{label} oversized source must fail closed: {error:#}"
        );
        assert!(
            !destination.join(base).exists(),
            "{label} must not publish an oversized source"
        );
        let preserved = rusqlite::Connection::open(source.join(base)).unwrap();
        assert_eq!(
            preserved
                .query_row(
                    "SELECT value FROM values_table ORDER BY rowid DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            label,
            "{label} must retain the legacy database after rejecting it"
        );
    }
}

#[cfg(unix)]
#[test]
fn sqlite_migration_rejects_corrupt_btree_for_all_layouts() {
    for (label, dest_rel, base) in LAYOUTS {
        let (_temp, source, destination) = layout_dirs(&format!("schema-{label}"), dest_rel);
        drop(live_sqlite_database(&source.join(base), label));
        corrupt_sqlite_btree(&destination.join(base));

        let error = migrate_generation(&source, &destination, base).expect_err(label);
        assert!(
            error.to_string().contains("SQLite") || error.to_string().contains("generation"),
            "{label} must reject a corrupt SQLite B-tree: {error:#}"
        );
        let legacy = rusqlite::Connection::open(source.join(base)).unwrap();
        assert_eq!(
            legacy
                .query_row(
                    "SELECT value FROM values_table ORDER BY rowid DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            label,
            "{label} must preserve the valid legacy database"
        );
    }
}

#[cfg(unix)]
#[test]
fn sqlite_migration_preserves_sidecar_after_identity_for_all_layouts() {
    for (label, dest_rel, base) in LAYOUTS {
        for suffix in SIDECARS {
            let (_temp, source, destination) =
                layout_dirs(&format!("sidecar-{label}{suffix}"), dest_rel);
            drop(live_sqlite_database(&source.join(base), label));
            let sidecar = destination.join(format!("{base}{suffix}"));
            std::fs::write(&sidecar, b"replacement-wal").unwrap();

            let result = migrate_generation(&source, &destination, base);
            assert!(
                sidecar.exists(),
                "{label}{suffix} replacement WAL must survive after the identity window; result={result:?}"
            );
            assert_eq!(
                std::fs::read(&sidecar).unwrap(),
                b"replacement-wal",
                "{label}{suffix} must not pathname-unlink a Hermes-writable sidecar"
            );
            if let Ok(()) = result {
                panic!(
                    "{label}{suffix} must not publish into a dest with a surviving foreign sidecar"
                );
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn sqlite_snapshot_fails_closed_on_exclusive_source_lock_for_all_layouts() {
    for (label, dest_rel, base) in LAYOUTS {
        let (_temp, source, destination) = layout_dirs(&format!("exclusive-{label}"), dest_rel);
        let holder = rusqlite::Connection::open(source.join(base)).unwrap();
        holder
            .execute_batch(
                "PRAGMA journal_mode=DELETE;
                 CREATE TABLE values_table (value TEXT NOT NULL);
                 INSERT INTO values_table (value) VALUES ('locked');
                 BEGIN EXCLUSIVE;",
            )
            .unwrap();

        let source_parent = std::fs::File::open(&source).unwrap();
        let destination_parent = std::fs::File::open(&destination).unwrap();
        let source_database = std::fs::File::open(source.join(base)).unwrap();
        let destination_path = destination.join(base);
        let (sender, receiver) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let result = super::super::super::super::hermes_paths::migrate_sqlite_generation(
                &source_parent,
                &destination_parent,
                &source_parent,
                std::ffi::OsStr::new(base),
                source_database,
                &destination_path,
            );
            sender.send(result).unwrap();
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let result = loop {
            match receiver.try_recv() {
                Ok(result) => break result,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "{label} exclusive source lock must fail closed in bound"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    panic!("{label} snapshot worker disconnected")
                }
            }
        };
        drop(holder);
        handle.join().unwrap();
        let error = result.expect_err(label);
        assert!(
            error.to_string().contains("timed out")
                || error.to_string().contains("SQLite")
                || error.to_string().contains("lock")
                || error.to_string().contains("busy")
                || error.to_string().contains("locked"),
            "{label} exclusive lock must be diagnosed: {error:#}"
        );
        assert!(
            !destination.join(base).exists(),
            "{label} must not publish a snapshot after exclusive-lock timeout"
        );
    }
}

#[cfg(unix)]
#[test]
fn sqlite_snapshot_failure_leaves_staging_empty_for_all_layouts() {
    for (label, dest_rel, base) in LAYOUTS {
        let (_temp, source, destination) = layout_dirs(&format!("staging-{label}"), dest_rel);
        drop(live_sqlite_database(&source.join(base), label));
        let _failure = AtomicCopyFailure::set();

        let error = migrate_generation(&source, &destination, base).expect_err(label);
        assert!(
            error.to_string().contains("SQLite")
                || error.to_string().contains("copy")
                || error.to_string().contains("generation")
                || error.to_string().contains("not writable"),
            "{label} snapshot failure must be diagnosed: {error:#}"
        );
        let leftovers = staging_snapshot_names(&source);
        assert!(
            leftovers.is_empty(),
            "{label} snapshot errors must leave no staging names: {leftovers:?}"
        );
        let staging = source.join(".csa-sqlite-staging");
        if staging.is_dir() {
            let remaining: Vec<_> = std::fs::read_dir(&staging)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert!(
                remaining.is_empty(),
                "{label} staging dir must be empty after snapshot failure: {remaining:?}"
            );
        }
        assert!(!destination.join(base).exists(), "{label} must not publish");
    }
}

#[cfg(unix)]
#[test]
fn sqlite_snapshot_unlink_failure_leaves_no_named_snapshot() {
    let (_temp, source, destination) = layout_dirs("unlink-failure", "runtime");
    drop(live_sqlite_database(&source.join("state.db"), "source"));
    let _hook = super::sqlite_3148_regression_tests::SqliteSnapshotCreatedHook::set(
        make_snapshot_directory_read_only,
    );

    let result = migrate_generation(&source, &destination, "state.db");
    let staging = source.join(".csa-sqlite-staging");
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700)).unwrap();

    result.unwrap();
    assert!(
        staging_snapshot_names(&source).is_empty(),
        "an unlink failure must not leave a named snapshot"
    );
}

#[cfg(unix)]
#[test]
fn sqlite_snapshot_process_death_leaves_no_named_snapshot() {
    if let Some(root) = std::env::var_os("CSA_SQLITE_SNAPSHOT_DEATH_ROOT") {
        let root = PathBuf::from(root);
        let source = root.join("legacy");
        let destination = root.join("runtime");
        let _hook = super::sqlite_3148_regression_tests::SqliteSourceOpenedHook::set(
            mark_snapshot_source_opened,
        );
        migrate_generation(&source, &destination, "state.db").unwrap();
        return;
    }

    let (temp, source, _destination) = layout_dirs("process-death", "runtime");
    let holder = rusqlite::Connection::open(source.join("state.db")).unwrap();
    holder
        .execute_batch(
            "PRAGMA journal_mode=DELETE;
             CREATE TABLE values_table (value TEXT NOT NULL);
             INSERT INTO values_table (value) VALUES ('locked');
             BEGIN EXCLUSIVE;",
        )
        .unwrap();
    drop(holder);
    let test_name = "isolation_plan::tests::hermes_review_3148_tests::sqlite_3148_tests::sqlite_3148_class_tests::sqlite_snapshot_process_death_leaves_no_named_snapshot";
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env("CSA_SQLITE_SNAPSHOT_DEATH_ROOT", temp.path())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let source_opened = source.join("backup-entered").exists();
        let _snapshot_open = std::fs::read_dir(format!("/proc/{}/fd", child.id()))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_link(entry.path()).ok())
            .any(|path| path.to_string_lossy().contains(".csa-sqlite-staging/"));
        if source_opened {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "child must enter backup with the snapshot FD open"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    child.kill().unwrap();
    child.wait().unwrap();

    assert!(
        staging_snapshot_names(&source).is_empty(),
        "process death must not leave a named snapshot"
    );
}
