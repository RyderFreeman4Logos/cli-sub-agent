//! Security and path traversal validation tests.
//!
//! Split from `package_tests.rs` to stay under the monolith-file limit.

use std::path::Path;

use tempfile::TempDir;

use super::*;

// ---------------------------------------------------------------------------
// Path traversal validation tests (P1-1)
// ---------------------------------------------------------------------------

#[test]
fn validate_package_name_accepts_valid_names() {
    assert!(validate_package_name("my-skill").is_ok());
    assert!(validate_package_name("audit_tool").is_ok());
    assert!(validate_package_name("Skill123").is_ok());
    assert!(validate_package_name("a").is_ok());
}

#[test]
fn validate_package_name_rejects_traversal() {
    assert!(validate_package_name("../../../etc").is_err());
    assert!(validate_package_name("..").is_err());
    assert!(validate_package_name(".").is_err());
    assert!(validate_package_name("foo/bar").is_err());
    assert!(validate_package_name("").is_err());
    assert!(validate_package_name("name with spaces").is_err());
}

#[test]
fn package_dir_rejects_traversal_in_name() {
    let store = Path::new("/store");
    let result = package_dir(store, "../../../etc", "aabbccdd");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("invalid package name"), "got: {err}");
}

#[test]
fn package_dir_rejects_traversal_in_commit() {
    let store = Path::new("/store");
    let result = package_dir(store, "safe-name", "../../foo");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("hex characters"), "got: {err}");
}

#[test]
fn package_dir_accepts_local_commit_key() {
    let store = Path::new("/store");
    let result = package_dir(store, "my-skill", "local");
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), Path::new("/store/my-skill/local"));
}

// ---------------------------------------------------------------------------
// migrate local-source skip test (P2-2)
// ---------------------------------------------------------------------------

#[test]
fn migrate_skips_local_and_migrates_git() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");

    // Pre-create a valid git checkout so migrate doesn't need real git.
    let checkout = package_dir(&store, "git-dep", "deadbeef").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Git Dep").unwrap();

    // Write legacy lockfile with both Local and Git source.
    let legacy = tmp.path().join(".weave").join("lock.toml");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let lockfile = Lockfile {
        package: vec![
            LockedPackage {
                name: "local-dep".to_string(),
                repo: String::new(),
                commit: String::new(),
                version: None,
                source_kind: SourceKind::Local,
                requested_version: None,
                resolved_ref: None,
            },
            LockedPackage {
                name: "git-dep".to_string(),
                repo: "https://github.com/org/git-dep.git".to_string(),
                commit: "deadbeef".to_string(),
                version: None,
                source_kind: SourceKind::Git,
                requested_version: None,
                resolved_ref: None,
            },
        ],
    };
    save_lockfile(&legacy, &lockfile).unwrap();

    let result = migrate(tmp.path(), &cache, &store).unwrap();
    match result {
        MigrateResult::Migrated {
            count,
            local_skipped,
            ..
        } => {
            assert_eq!(count, 2, "total package count");
            assert_eq!(local_skipped, 1, "local packages skipped");
        }
        other => panic!("expected Migrated, got: {other:?}"),
    }

    // New lockfile must exist with both packages preserved.
    let new_path = lockfile_path(tmp.path());
    assert!(new_path.is_file());
    let loaded = load_lockfile(&new_path).unwrap();
    assert_eq!(loaded.package.len(), 2);
}
