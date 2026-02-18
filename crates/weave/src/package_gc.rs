//! Garbage-collect unreferenced checkouts from the global package store.
//!
//! Split from `package.rs` to stay under the monolith-file limit.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};

use super::{Lockfile, SourceKind, load_project_lockfile};

/// Result of a `weave gc` operation.
#[derive(Debug, PartialEq)]
pub struct GcResult {
    /// Names of removed checkout directories (format: `name/prefix`).
    pub removed: Vec<String>,
    /// Total bytes freed by removing unreferenced checkouts.
    pub freed_bytes: u64,
}

/// Garbage-collect unreferenced checkouts from the global package store.
///
/// Walks `store_root` looking for `<name>/<commit_prefix>/` directories.
/// Any checkout not referenced by the current project's lockfile is
/// removed (or just listed when `dry_run` is true).
pub fn gc(project_root: &Path, store_root: &Path, dry_run: bool) -> Result<GcResult> {
    let referenced = build_reference_set(project_root)?;

    let mut removed = Vec::new();
    let mut freed_bytes: u64 = 0;

    if !store_root.is_dir() {
        // Nothing to collect — store does not exist yet.
        return Ok(GcResult {
            removed,
            freed_bytes,
        });
    }

    // Walk: store_root/<name>/<prefix>/
    let name_entries = std::fs::read_dir(store_root)
        .with_context(|| format!("failed to read global store at {}", store_root.display()))?;

    for name_entry in name_entries {
        let name_entry = name_entry?;
        if !name_entry.file_type()?.is_dir() {
            continue;
        }
        let pkg_name = name_entry.file_name();
        let pkg_name_str = pkg_name.to_string_lossy();

        let prefix_entries = std::fs::read_dir(name_entry.path()).with_context(|| {
            format!(
                "failed to read store directory {}",
                name_entry.path().display()
            )
        })?;

        for prefix_entry in prefix_entries {
            let prefix_entry = prefix_entry?;
            if !prefix_entry.file_type()?.is_dir() {
                continue;
            }
            let prefix = prefix_entry.file_name();
            let prefix_str = prefix.to_string_lossy();

            let key = format!("{pkg_name_str}/{prefix_str}");
            if referenced.contains(&key) {
                continue;
            }

            let checkout_path = prefix_entry.path();
            let size = dir_size(&checkout_path);

            if dry_run {
                removed.push(key);
                freed_bytes += size;
            } else {
                std::fs::remove_dir_all(&checkout_path)
                    .with_context(|| format!("failed to remove {}", checkout_path.display()))?;
                removed.push(key);
                freed_bytes += size;

                // Remove the parent name dir if now empty.
                let parent = name_entry.path();
                if is_dir_empty(&parent) {
                    let _ = std::fs::remove_dir(&parent);
                }
            }
        }
    }

    Ok(GcResult {
        removed,
        freed_bytes,
    })
}

/// Build the set of `name/prefix` keys that are referenced by the lockfile.
fn build_reference_set(project_root: &Path) -> Result<HashSet<String>> {
    let lockfile = load_project_lockfile(project_root).unwrap_or(Lockfile {
        package: Vec::new(),
    });

    let mut refs = HashSet::new();
    for pkg in &lockfile.package {
        let commit_key = if pkg.source_kind == SourceKind::Local {
            "local".to_string()
        } else if pkg.commit.is_empty() {
            continue;
        } else {
            // package_dir uses commit[..min(8, len)] as prefix
            let prefix_len = pkg.commit.len().min(8);
            pkg.commit[..prefix_len].to_string()
        };
        let key = format!("{}/{}", pkg.name, commit_key);
        refs.insert(key);
    }

    Ok(refs)
}

/// Recursively compute the total size of a directory in bytes.
fn dir_size(path: &Path) -> u64 {
    let mut total: u64 = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_dir() {
                    total += dir_size(&entry.path());
                } else {
                    total += meta.len();
                }
            }
        }
    }
    total
}

/// Check whether a directory is empty.
fn is_dir_empty(path: &Path) -> bool {
    match std::fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_none(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{LockedPackage, Lockfile, SourceKind, save_lockfile};

    /// Helper: create a fake checkout directory with a file in it.
    fn create_checkout(store: &Path, name: &str, prefix: &str) {
        let dir = store.join(name).join(prefix);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "# test skill\n").unwrap();
    }

    /// Helper: write a minimal lockfile referencing given packages.
    fn write_lockfile(project: &Path, packages: Vec<LockedPackage>) {
        let lockfile = Lockfile { package: packages };
        let lock_path = project.join("weave.lock");
        save_lockfile(&lock_path, &lockfile).unwrap();
    }

    fn make_pkg(name: &str, commit: &str) -> LockedPackage {
        LockedPackage {
            name: name.to_string(),
            repo: format!("https://github.com/test/{name}.git"),
            commit: commit.to_string(),
            version: None,
            source_kind: SourceKind::Git,
            requested_version: None,
            resolved_ref: None,
        }
    }

    fn make_local_pkg(name: &str) -> LockedPackage {
        LockedPackage {
            name: name.to_string(),
            repo: String::new(),
            commit: String::new(),
            version: None,
            source_kind: SourceKind::Local,
            requested_version: None,
            resolved_ref: None,
        }
    }

    #[test]
    fn test_gc_removes_unreferenced_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&project).unwrap();

        // Create two checkouts: one referenced, one not.
        create_checkout(&store, "alpha", "aabbccdd");
        create_checkout(&store, "orphan", "11223344");

        // Lockfile only references alpha.
        write_lockfile(&project, vec![make_pkg("alpha", "aabbccddee112233")]);

        let result = gc(&project, &store, false).unwrap();
        assert_eq!(result.removed, vec!["orphan/11223344"]);
        assert!(result.freed_bytes > 0);

        // Verify orphan was actually removed.
        assert!(!store.join("orphan").join("11223344").exists());
        // Verify referenced checkout preserved.
        assert!(store.join("alpha").join("aabbccdd").exists());
    }

    #[test]
    fn test_gc_dry_run_does_not_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&project).unwrap();

        create_checkout(&store, "alpha", "aabbccdd");
        create_checkout(&store, "orphan", "11223344");

        write_lockfile(&project, vec![make_pkg("alpha", "aabbccddee112233")]);

        let result = gc(&project, &store, true).unwrap();
        assert_eq!(result.removed, vec!["orphan/11223344"]);
        assert!(result.freed_bytes > 0);

        // Dry run: orphan should still exist.
        assert!(store.join("orphan").join("11223344").exists());
    }

    #[test]
    fn test_gc_preserves_referenced_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&project).unwrap();

        create_checkout(&store, "alpha", "aabbccdd");

        write_lockfile(&project, vec![make_pkg("alpha", "aabbccddee112233")]);

        let result = gc(&project, &store, false).unwrap();
        assert!(result.removed.is_empty());
        assert_eq!(result.freed_bytes, 0);
        assert!(store.join("alpha").join("aabbccdd").exists());
    }

    #[test]
    fn test_gc_preserves_local_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&project).unwrap();

        create_checkout(&store, "local-skill", "local");

        write_lockfile(&project, vec![make_local_pkg("local-skill")]);

        let result = gc(&project, &store, false).unwrap();
        assert!(result.removed.is_empty());
        assert!(store.join("local-skill").join("local").exists());
    }

    #[test]
    fn test_gc_empty_store() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&project).unwrap();

        write_lockfile(&project, vec![]);

        let result = gc(&project, &store, false).unwrap();
        assert!(result.removed.is_empty());
        assert_eq!(result.freed_bytes, 0);
    }

    #[test]
    fn test_gc_no_lockfile_removes_all() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&project).unwrap();
        // No lockfile written — all checkouts are unreferenced.

        create_checkout(&store, "orphan1", "aaaabbbb");
        create_checkout(&store, "orphan2", "ccccdddd");

        let result = gc(&project, &store, false).unwrap();
        assert_eq!(result.removed.len(), 2);
        assert!(!store.join("orphan1").exists());
        assert!(!store.join("orphan2").exists());
    }

    #[test]
    fn test_gc_removes_empty_parent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&project).unwrap();

        // Single checkout under "orphan" — after gc, parent should be removed.
        create_checkout(&store, "orphan", "aabbccdd");

        write_lockfile(&project, vec![]);

        let result = gc(&project, &store, false).unwrap();
        assert_eq!(result.removed, vec!["orphan/aabbccdd"]);
        // Parent directory should be cleaned up.
        assert!(!store.join("orphan").exists());
    }
}
