//! Generation-ownership and after-identity publish/cleanup regressions (#3148).

use super::*;
use std::path::Path;

#[cfg(unix)]
struct SqliteDestinationObservedHook;

#[cfg(unix)]
impl SqliteDestinationObservedHook {
    fn set(inject: fn(&Path)) -> Self {
        super::super::super::super::hermes_paths::AFTER_SQLITE_DESTINATION_OBSERVED
            .with(|hook| hook.set(Some(inject)));
        Self
    }
}

#[cfg(unix)]
impl Drop for SqliteDestinationObservedHook {
    fn drop(&mut self) {
        super::super::super::super::hermes_paths::AFTER_SQLITE_DESTINATION_OBSERVED
            .with(|hook| hook.set(None));
    }
}

#[cfg(unix)]
struct RemoveFileIdentityHook;

#[cfg(unix)]
impl RemoveFileIdentityHook {
    fn set(inject: fn(&Path)) -> Self {
        super::super::super::super::readable::AFTER_REMOVE_FILE_IDENTITY
            .with(|hook| hook.set(Some(inject)));
        Self
    }
}

#[cfg(unix)]
impl Drop for RemoveFileIdentityHook {
    fn drop(&mut self) {
        super::super::super::super::readable::AFTER_REMOVE_FILE_IDENTITY
            .with(|hook| hook.set(None));
    }
}

#[cfg(unix)]
struct AtomicCopyIdentityHook;

#[cfg(unix)]
impl AtomicCopyIdentityHook {
    fn set(inject: fn(&Path)) -> Self {
        super::super::super::super::readable::AFTER_ATOMIC_COPY_IDENTITY
            .with(|hook| hook.set(Some(inject)));
        Self
    }
}

#[cfg(unix)]
impl Drop for AtomicCopyIdentityHook {
    fn drop(&mut self) {
        super::super::super::super::readable::AFTER_ATOMIC_COPY_IDENTITY
            .with(|hook| hook.set(None));
    }
}

fn resolved_leaf(path: &Path) -> std::path::PathBuf {
    let parent = path.parent().unwrap();
    parent
        .read_link()
        .unwrap_or_else(|_| parent.to_path_buf())
        .join(path.file_name().unwrap())
}

fn inject_nonsqlite_generation(db_path: &Path) {
    let db_path = resolved_leaf(db_path);
    std::fs::write(&db_path, b"not-sqlite").unwrap();
    std::fs::write(
        db_path.with_file_name(format!(
            "{}-wal",
            db_path.file_name().unwrap().to_string_lossy()
        )),
        b"live-wal",
    )
    .unwrap();
}

fn inject_live_sqlite_generation(db_path: &Path) {
    let db_path = resolved_leaf(db_path);
    drop(live_sqlite_database(&db_path, "live"));
    std::fs::write(
        db_path.with_file_name(format!(
            "{}-wal",
            db_path.file_name().unwrap().to_string_lossy()
        )),
        b"live-wal",
    )
    .unwrap();
}

fn replace_after_identity(path: &Path) {
    if path.exists() {
        let parked = path.with_file_name(".csa-after-identity-parked");
        std::fs::rename(path, parked).unwrap();
    }
    std::fs::write(path, b"replacement-after-identity").unwrap();
}

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

#[cfg(unix)]
#[test]
fn sqlite_migration_rejects_nonsqlite_winner_injected_after_absent_check() {
    let temp = tempfile::Builder::new()
        .prefix("hermes-nonsqlite-winner-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let source = temp.path().join("legacy");
    let destination = temp.path().join("runtime");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    drop(live_sqlite_database(&source.join("state.db"), "source"));
    let _hook = SqliteDestinationObservedHook::set(inject_nonsqlite_generation);

    let error = migrate_generation(&source, &destination, "state.db")
        .expect_err("a non-SQLite winner must not be accepted");

    assert!(
        error.to_string().contains("SQLite") || error.to_string().contains("generation"),
        "non-SQLite winner must be diagnosed: {error:#}"
    );
    assert_eq!(
        std::fs::read(destination.join("state.db")).unwrap(),
        b"not-sqlite"
    );
    assert_eq!(
        std::fs::read(destination.join("state.db-wal")).unwrap(),
        b"live-wal"
    );
}

#[cfg(unix)]
#[test]
fn sqlite_migration_preserves_live_sidecar_injected_after_absent_check() {
    let temp = tempfile::Builder::new()
        .prefix("hermes-live-sidecar-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let source = temp.path().join("legacy");
    let destination = temp.path().join("runtime");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    drop(live_sqlite_database(&source.join("state.db"), "source"));
    let _hook = SqliteDestinationObservedHook::set(inject_live_sqlite_generation);

    let result = migrate_generation(&source, &destination, "state.db");
    assert!(
        destination.join("state.db-wal").exists(),
        "a live WAL injected after the path-absent check must not be unlinked; result={result:?}"
    );
    if result.is_ok() {
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
            "live"
        );
    }
}

#[cfg(unix)]
#[test]
fn sqlite_migration_generation_ownership_covers_root_flat_direct_nested() {
    let layouts = [
        ("root", "runtime", "state.db"),
        ("flat", "runtime", "state.flat.db"),
        ("direct", "runtime/direct", "state.db"),
        ("nested", "runtime/profiles/nested", "state.db"),
    ];
    for (label, dest_rel, base) in layouts {
        let temp = tempfile::Builder::new()
            .prefix(&format!("hermes-generation-{label}-"))
            .tempdir_in("/var/tmp")
            .unwrap();
        let source = temp.path().join("legacy");
        let destination = temp.path().join(dest_rel);
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir_all(&destination).unwrap();
        drop(live_sqlite_database(&source.join(base), label));
        let _hook = SqliteDestinationObservedHook::set(inject_nonsqlite_generation);

        let error = migrate_generation(&source, &destination, base).expect_err(label);
        assert!(
            error.to_string().contains("SQLite") || error.to_string().contains("generation"),
            "{label} must reject a non-SQLite winner: {error:#}"
        );
        assert_eq!(
            std::fs::read(destination.join(format!("{base}-wal"))).unwrap(),
            b"live-wal",
            "{label} must keep the injected live sidecar"
        );
        assert_eq!(
            std::fs::read(destination.join(base)).unwrap(),
            b"not-sqlite",
            "{label} must not replace the injected winner with a published snapshot"
        );
    }
}

#[cfg(unix)]
#[test]
fn remove_file_if_same_fails_closed_after_identity_check() {
    let temp = tempfile::Builder::new()
        .prefix("hermes-unlink-after-identity-")
        .tempdir_in("/var/tmp")
        .unwrap();
    std::fs::write(temp.path().join("leaf"), b"original").unwrap();
    let parent = std::fs::File::open(temp.path()).unwrap();
    let expected = std::fs::File::open(temp.path().join("leaf")).unwrap();
    let _hook = RemoveFileIdentityHook::set(replace_after_identity);

    let error = super::super::super::super::readable::remove_file_if_same(
        &parent,
        std::ffi::OsStr::new("leaf"),
        &expected,
    )
    .expect_err("pathname unlink after identity must fail closed");

    assert!(
        error.to_string().contains("changed")
            || error.to_string().contains("unlink")
            || error.to_string().contains("identity"),
        "after-identity replacement must be diagnosed: {error}"
    );
    assert_eq!(
        std::fs::read(temp.path().join("leaf")).unwrap(),
        b"replacement-after-identity"
    );
    assert!(temp.path().join(".csa-after-identity-parked").exists());
}

#[cfg(unix)]
#[test]
fn atomic_copy_fails_closed_after_identity_check() {
    let temp = tempfile::Builder::new()
        .prefix("hermes-copy-after-identity-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let source_path = temp.path().join("source");
    let destination_path = temp.path().join("runtime");
    std::fs::write(&source_path, b"source").unwrap();
    std::fs::create_dir(&destination_path).unwrap();
    let source = std::fs::File::open(source_path).unwrap();
    let destination_parent = std::fs::File::open(&destination_path).unwrap();
    let _hook = AtomicCopyIdentityHook::set(replace_after_identity);

    let error = super::super::super::super::readable::copy_pinned_file_atomic(
        &source,
        &destination_parent,
        std::ffi::OsStr::new("state.db"),
    )
    .expect_err("temp replacement after identity must fail closed");

    assert!(
        error.to_string().contains("changed")
            || error.to_string().contains("exist")
            || error.to_string().contains("identity")
            || error.to_string().contains("publication"),
        "after-identity copy replacement must be diagnosed: {error}"
    );
    assert_eq!(
        std::fs::read(destination_path.join("state.db")).unwrap(),
        b"replacement-after-identity"
    );
}
