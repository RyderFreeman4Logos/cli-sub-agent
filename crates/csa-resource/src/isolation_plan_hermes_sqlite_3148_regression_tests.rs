//! Focused SQLite descriptor and lock regressions for #3148.

use super::*;
use std::path::Path;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
pub(super) struct SqliteSourceOpenedHook;

#[cfg(unix)]
impl SqliteSourceOpenedHook {
    pub(super) fn set(inject: fn(&Path)) -> Self {
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
pub(super) struct SqliteSnapshotCreatedHook;

#[cfg(unix)]
impl SqliteSnapshotCreatedHook {
    pub(super) fn set(inject: fn(&Path)) -> Self {
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
struct SqliteAtomicCopyWrittenHook;

#[cfg(unix)]
impl SqliteAtomicCopyWrittenHook {
    fn set(inject: fn(&Path)) -> Self {
        super::super::super::super::readable::AFTER_ATOMIC_COPY_WRITTEN
            .with(|hook| hook.set(Some(inject)));
        Self
    }
}

#[cfg(unix)]
impl Drop for SqliteAtomicCopyWrittenHook {
    fn drop(&mut self) {
        super::super::super::super::readable::AFTER_ATOMIC_COPY_WRITTEN.with(|hook| hook.set(None));
    }
}

#[cfg(unix)]
struct SqliteRemoveFileIdentityHook;

#[cfg(unix)]
impl SqliteRemoveFileIdentityHook {
    fn set(inject: fn(&Path)) -> Self {
        super::super::super::super::readable::AFTER_REMOVE_FILE_IDENTITY
            .with(|hook| hook.set(Some(inject)));
        Self
    }
}

#[cfg(unix)]
impl Drop for SqliteRemoveFileIdentityHook {
    fn drop(&mut self) {
        super::super::super::super::readable::AFTER_REMOVE_FILE_IDENTITY
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
    let snapshot_path =
        std::fs::read_link(snapshot_path).unwrap_or_else(|_| snapshot_path.to_path_buf());
    if snapshot_path.exists() {
        let parked = snapshot_path.with_file_name(".csa-snapshot-parked");
        std::fs::rename(&snapshot_path, parked).unwrap();
    }
    std::fs::write(&snapshot_path, []).unwrap();
}

fn create_destination_winner_with_sidecar(copy_path: &Path) {
    let copy_path = std::fs::read_link(copy_path).unwrap_or_else(|_| copy_path.to_path_buf());
    let parent = copy_path.parent().unwrap();
    let dest = parent
        .read_link()
        .unwrap_or_else(|_| parent.to_path_buf())
        .join(copy_path.file_name().unwrap());
    std::fs::write(&dest, b"winner").unwrap();
    std::fs::write(
        dest.with_file_name(format!(
            "{}-wal",
            dest.file_name().unwrap().to_string_lossy()
        )),
        b"winner-wal",
    )
    .unwrap();
}

fn replace_sidecar_with_replacement(sidecar_path: &Path) {
    let sidecar_path =
        std::fs::read_link(sidecar_path).unwrap_or_else(|_| sidecar_path.to_path_buf());
    if sidecar_path.exists() {
        let parked = sidecar_path.with_file_name(".csa-sidecar-parked");
        std::fs::rename(&sidecar_path, parked).unwrap();
    }
    std::fs::write(&sidecar_path, b"replacement").unwrap();
}

#[cfg(unix)]
fn replace_atomic_copy_with_empty_inode(copy_path: &Path) {
    if copy_path.exists() {
        let parked = copy_path.with_file_name(".csa-copy-parked");
        std::fs::rename(copy_path, parked).unwrap();
    }
    std::fs::write(copy_path, []).unwrap();
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
    .expect("pinned snapshot FD must survive pathname replacement");
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

#[cfg(unix)]
#[test]
fn sqlite_atomic_copy_preserves_replaced_temporary_inode() {
    let temp = tempfile::Builder::new()
        .prefix("hermes-sqlite-copy-replace-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let source_path = temp.path().join("source");
    let destination_path = temp.path().join("runtime");
    std::fs::write(&source_path, b"source").unwrap();
    std::fs::create_dir(&destination_path).unwrap();
    let source = std::fs::File::open(source_path).unwrap();
    let destination_parent = std::fs::File::open(&destination_path).unwrap();
    let _hook = SqliteAtomicCopyWrittenHook::set(replace_atomic_copy_with_empty_inode);

    let error = super::super::super::super::readable::copy_pinned_file_atomic(
        &source,
        &destination_parent,
        std::ffi::OsStr::new("state.db"),
    )
    .expect_err("temporary pathname replacement must fail closed");

    assert!(
        error.to_string().contains("temporary SQLite copy changed")
            || error.to_string().contains("exist")
            || error.kind() == std::io::ErrorKind::AlreadyExists,
        "temporary replacement must be diagnosed: {error}"
    );
    assert_eq!(
        std::fs::read(destination_path.join("state.db")).unwrap(),
        b""
    );
}

#[cfg(unix)]
#[test]
fn sqlite_migration_preserves_replaced_sidecar_inode() {
    let temp = tempfile::Builder::new()
        .prefix("hermes-sqlite-sidecar-replace-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let source = temp.path().join("legacy");
    let destination = temp.path().join("runtime");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    let source_connection = live_sqlite_database(&source.join("state.db"), "source");
    drop(source_connection);
    std::fs::write(destination.join("state.db-wal"), b"original").unwrap();
    let source_parent = std::fs::File::open(&source).unwrap();
    let destination_parent = std::fs::File::open(&destination).unwrap();
    let source_database = std::fs::File::open(source.join("state.db")).unwrap();
    let _hook = SqliteRemoveFileIdentityHook::set(replace_sidecar_with_replacement);

    let error = super::super::super::super::hermes_paths::migrate_sqlite_generation(
        &source_parent,
        &destination_parent,
        &source_parent,
        std::ffi::OsStr::new("state.db"),
        source_database,
        &destination.join("state.db"),
    )
    .expect_err("sidecar pathname replacement must fail closed");

    assert!(
        error.to_string().contains("incomplete SQLite generation")
            || error
                .to_string()
                .contains("pinned temporary file changed before cleanup")
            || error.to_string().contains("identity")
            || error.to_string().contains("unlink"),
        "sidecar replacement must be diagnosed: {error:#}"
    );
    assert_eq!(
        std::fs::read(destination.join("state.db-wal")).unwrap(),
        b"original"
    );
    assert!(!destination.join("state.db").exists());
}

#[cfg(unix)]
#[test]
fn sqlite_migration_rejects_unsafe_publication_winner() {
    let temp = tempfile::Builder::new()
        .prefix("hermes-sqlite-publication-winner-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let source = temp.path().join("legacy");
    let destination = temp.path().join("runtime");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    let source_connection = live_sqlite_database(&source.join("state.db"), "source");
    drop(source_connection);
    let source_parent = std::fs::File::open(&source).unwrap();
    let destination_parent = std::fs::File::open(&destination).unwrap();
    let source_database = std::fs::File::open(source.join("state.db")).unwrap();
    let _hook = SqliteAtomicCopyWrittenHook::set(create_destination_winner_with_sidecar);

    let error = super::super::super::super::hermes_paths::migrate_sqlite_generation(
        &source_parent,
        &destination_parent,
        &source_parent,
        std::ffi::OsStr::new("state.db"),
        source_database,
        &destination.join("state.db"),
    )
    .expect_err("an unsafe publication winner must fail closed");

    assert!(
        error.to_string().contains("publication winner"),
        "unsafe winner must be diagnosed: {error:#}"
    );
    assert_eq!(
        std::fs::read(destination.join("state.db")).unwrap(),
        b"winner"
    );
    assert_eq!(
        std::fs::read(destination.join("state.db-wal")).unwrap(),
        b"winner-wal"
    );
}
