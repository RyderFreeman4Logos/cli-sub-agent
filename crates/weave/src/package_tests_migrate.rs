//! Migration-related tests for the `package` module.
//!
//! Split from `package_tests.rs` to stay under the monolith-file limit.

use tempfile::TempDir;

use super::*;

#[test]
fn migrate_nothing_when_no_legacy_lockfile() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");
    let result = migrate(tmp.path(), &cache, &store).unwrap();
    assert_eq!(result, MigrateResult::NothingToMigrate);
}

#[test]
fn migrate_detects_orphaned_weave_deps() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");
    // Create .weave/deps/ without lock.toml
    let deps_dir = tmp.path().join(".weave").join("deps");
    std::fs::create_dir_all(&deps_dir).unwrap();
    std::fs::write(deps_dir.join("some-package"), "placeholder").unwrap();

    let result = migrate(tmp.path(), &cache, &store).unwrap();
    match &result {
        MigrateResult::OrphanedDirs(dirs) => {
            assert_eq!(dirs.len(), 1);
            assert!(
                dirs[0].description.contains(".weave"),
                "expected .weave mention, got: {}",
                dirs[0].description
            );
            assert!(
                dirs[0].cleanup_hint.contains("rm -rf"),
                "expected cleanup hint"
            );
        }
        other => panic!("expected OrphanedDirs, got: {other:?}"),
    }
}

#[test]
fn migrate_detects_legacy_csa_patterns() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");
    // Create .csa/patterns/ directory
    let patterns = tmp.path().join(".csa").join("patterns");
    std::fs::create_dir_all(&patterns).unwrap();

    let result = migrate(tmp.path(), &cache, &store).unwrap();
    match &result {
        MigrateResult::OrphanedDirs(dirs) => {
            assert!(
                dirs.iter().any(|d| d.description.contains(".csa/patterns")),
                "expected .csa/patterns mention, got: {dirs:?}"
            );
        }
        other => panic!("expected OrphanedDirs, got: {other:?}"),
    }
}

#[test]
fn migrate_detects_both_orphaned_dirs() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");
    // Create both orphaned directories
    let deps = tmp.path().join(".weave").join("deps");
    std::fs::create_dir_all(&deps).unwrap();
    let patterns = tmp.path().join(".csa").join("patterns");
    std::fs::create_dir_all(&patterns).unwrap();

    let result = migrate(tmp.path(), &cache, &store).unwrap();
    match &result {
        MigrateResult::OrphanedDirs(dirs) => {
            assert_eq!(dirs.len(), 2, "expected 2 legacy dirs, got: {dirs:?}");
        }
        other => panic!("expected OrphanedDirs, got: {other:?}"),
    }
}

#[test]
fn migrate_ignores_weave_dir_with_non_deps_content() {
    // If .weave/ contains files other than deps/, the orphan detection
    // should still report it (as orphaned deps), not suggest removing .weave/.
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");
    let weave_dir = tmp.path().join(".weave");
    std::fs::create_dir_all(weave_dir.join("deps")).unwrap();
    std::fs::write(weave_dir.join("config.toml"), "some config").unwrap();

    let result = migrate(tmp.path(), &cache, &store).unwrap();
    match &result {
        MigrateResult::OrphanedDirs(dirs) => {
            // Should report orphaned deps but NOT suggest removing whole .weave/
            // because it contains other files (config.toml).
            assert_eq!(dirs.len(), 1);
            assert!(
                dirs[0].cleanup_hint.contains(".weave/deps"),
                "should suggest removing deps/ only, got: {}",
                dirs[0].cleanup_hint
            );
        }
        other => panic!("expected OrphanedDirs, got: {other:?}"),
    }
}

#[test]
fn migrate_already_migrated_when_weave_lock_exists() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");
    let new = lockfile_path(tmp.path());
    std::fs::write(&new, "").unwrap();

    let result = migrate(tmp.path(), &cache, &store).unwrap();
    assert_eq!(result, MigrateResult::AlreadyMigrated);
}

#[test]
fn migrate_creates_weave_lock_from_legacy() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");
    let checkout = package_dir(&store, "test-skill", "abc12345").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Test").unwrap();
    let legacy = tmp.path().join(".weave").join("lock.toml");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let lockfile = Lockfile::with_packages(vec![LockedPackage {
        name: "test-skill".to_string(),
        repo: String::new(),
        commit: String::new(),
        version: None,
        source_kind: SourceKind::Local,
        requested_version: None,
        resolved_ref: None,
    }]);
    save_lockfile(&legacy, &lockfile).unwrap();

    let result = migrate(tmp.path(), &cache, &store).unwrap();
    assert!(
        matches!(result, MigrateResult::Migrated { count: 1, .. }),
        "expected Migrated with 1 package, got: {result:?}"
    );
    let new_path = lockfile_path(tmp.path());
    assert!(new_path.is_file(), "weave.lock should be created");
    let loaded = load_lockfile(&new_path).unwrap();
    assert_eq!(loaded.package.len(), 1);
    assert_eq!(loaded.package[0].name, "test-skill");
}

#[test]
fn migrate_skips_valid_checkout_in_global_store() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");
    let checkout = package_dir(&store, "pre-existing", "deadbeef").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Pre-existing").unwrap();
    let legacy = tmp.path().join(".weave").join("lock.toml");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let lockfile = Lockfile::with_packages(vec![LockedPackage {
        name: "pre-existing".to_string(),
        repo: "https://example.com/pre-existing.git".to_string(),
        commit: "deadbeef".to_string(),
        version: None,
        source_kind: SourceKind::Git,
        requested_version: None,
        resolved_ref: None,
    }]);
    save_lockfile(&legacy, &lockfile).unwrap();
    let result = migrate(tmp.path(), &cache, &store).unwrap();
    assert!(
        matches!(
            result,
            MigrateResult::Migrated {
                count: 1,
                checkouts: 0,
                ..
            }
        ),
        "expected Migrated(count=1, checkouts=0) since checkout valid, got: {result:?}"
    );
    assert!(lockfile_path(tmp.path()).is_file());
}
