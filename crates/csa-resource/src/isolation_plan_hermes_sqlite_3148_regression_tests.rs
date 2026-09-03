//! Focused SQLite descriptor and lock regressions for #3148.

use super::*;
use std::path::Path;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
struct SqliteSourceOpenedHook;

#[cfg(unix)]
impl SqliteSourceOpenedHook {
    fn set(inject: fn(&Path)) -> Self {
        super::super::super::super::hermes_paths::AFTER_SQLITE_SOURCE_OPENED
            .with(|hook| hook.set(Some(inject)));
        Self
    }
}

#[cfg(unix)]
impl Drop for SqliteSourceOpenedHook {
    fn drop(&mut self) {
        super::super::super::super::hermes_paths::AFTER_SQLITE_SOURCE_OPENED
            .with(|hook| hook.set(None));
    }
}

#[cfg(unix)]
struct SqliteSnapshotCreatedHook;

#[cfg(unix)]
impl SqliteSnapshotCreatedHook {
    fn set(inject: fn(&Path)) -> Self {
        super::super::super::super::hermes_paths::AFTER_SQLITE_SNAPSHOT_CREATED
            .with(|hook| hook.set(Some(inject)));
        Self
    }
}

#[cfg(unix)]
impl Drop for SqliteSnapshotCreatedHook {
    fn drop(&mut self) {
        super::super::super::super::hermes_paths::AFTER_SQLITE_SNAPSHOT_CREATED
            .with(|hook| hook.set(None));
    }
}

#[cfg(unix)]
fn replace_source_with_b_then_restore_a(source_path: &Path) {
    let source_path = std::fs::read_link(source_path).unwrap();
    let parent = source_path.parent().unwrap();
    let original = parent.join(".csa-aba-original");
    let replacement = parent.join(".csa-aba-replacement");
    let parked = parent.join(".csa-aba-parked");
    std::fs::rename(&source_path, &original).unwrap();
    std::fs::rename(&replacement, &source_path).unwrap();
    std::fs::rename(&source_path, &parked).unwrap();
    std::fs::rename(original, source_path).unwrap();
}

#[cfg(unix)]
fn replace_snapshot_with_empty_inode(snapshot_path: &Path) {
    let snapshot_path = std::fs::read_link(snapshot_path).unwrap();
    let parked = snapshot_path.with_file_name(".csa-snapshot-parked");
    std::fs::rename(&snapshot_path, parked).unwrap();
    std::fs::write(snapshot_path, []).unwrap();
}

#[cfg(unix)]
#[test]
fn sqlite_migration_uses_pinned_source_fd_across_aba_replacement() {
    let temp = tempfile::Builder::new()
        .prefix("hermes-sqlite-aba-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let source = temp.path().join("legacy");
    let destination = temp.path().join("runtime");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    let source_connection = live_sqlite_database(&source.join("state.db"), "A");
    let replacement_connection = live_sqlite_database(&source.join(".csa-aba-replacement"), "B");
    replacement_connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    drop(replacement_connection);
    let source_parent = std::fs::File::open(&source).unwrap();
    let source_database = std::fs::File::open(source.join("state.db")).unwrap();
    let destination_parent = std::fs::File::open(&destination).unwrap();
    let _hook = SqliteSourceOpenedHook::set(replace_source_with_b_then_restore_a);

    super::super::super::super::hermes_paths::migrate_sqlite_generation(
        &source_parent,
        &destination_parent,
        &source_parent,
        std::ffi::OsStr::new("state.db"),
        source_database,
        &destination.join("state.db"),
    )
    .unwrap();
    drop(source_connection);

    let migrated = rusqlite::Connection::open(destination.join("state.db")).unwrap();
    assert_eq!(
        migrated
            .query_row(
                "SELECT value FROM values_table ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "A"
    );
}

#[cfg(unix)]
#[test]
fn sqlite_migration_uses_pinned_snapshot_fd_across_name_replacement() {
    let temp = tempfile::Builder::new()
        .prefix("hermes-sqlite-snapshot-replace-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let source = temp.path().join("legacy");
    let destination = temp.path().join("runtime");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    let source_connection = live_sqlite_database(&source.join("state.db"), "source");
    let source_parent = std::fs::File::open(&source).unwrap();
    let destination_parent = std::fs::File::open(&destination).unwrap();
    let source_database = std::fs::File::open(source.join("state.db")).unwrap();
    let _hook = SqliteSnapshotCreatedHook::set(replace_snapshot_with_empty_inode);

    super::super::super::super::hermes_paths::migrate_sqlite_generation(
        &source_parent,
        &destination_parent,
        &source_parent,
        std::ffi::OsStr::new("state.db"),
        source_database,
        &destination.join("state.db"),
    )
    .unwrap();
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
        "source"
    );
}

#[cfg(unix)]
#[test]
fn sqlite_migration_fails_closed_when_independent_lock_holder_wedges() {
    let temp = tempfile::Builder::new()
        .prefix("hermes-sqlite-lock-timeout-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let source = temp.path().join("legacy");
    let destination = temp.path().join("runtime");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    let source_connection = live_sqlite_database(&source.join("state.db"), "source");
    let holder_parent = std::fs::File::open(&source).unwrap();
    let holder =
        super::super::super::super::hermes_paths::acquire_sqlite_generation_lock(&holder_parent)
            .unwrap();
    let source_parent = std::fs::File::open(&source).unwrap();
    let destination_parent = std::fs::File::open(&destination).unwrap();
    let source_database = std::fs::File::open(source.join("state.db")).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let result = super::super::super::super::hermes_paths::migrate_sqlite_generation(
            &source_parent,
            &destination_parent,
            &holder_parent,
            std::ffi::OsStr::new("state.db"),
            source_database,
            &destination.join("state.db"),
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
                    "lock contention must have a bounded result"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                panic!("migration worker disconnected")
            }
        }
    };
    drop(holder);
    handle.join().unwrap();
    drop(source_connection);
    let error = result.expect_err("a wedged independent holder must fail closed");
    assert!(
        error
            .to_string()
            .contains("timed out waiting for SQLite generation lock"),
        "lock timeout must be explicit: {error:#}"
    );
}
