//! Migrate from legacy `.weave/lock.toml` to `weave.lock` and global store.
//!
//! Split from `package.rs` to stay under the monolith-file limit.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{
    SourceKind, checkout_to, ensure_cached, is_checkout_valid, legacy_lockfile_path, load_lockfile,
    lockfile_path, package_dir, save_lockfile,
};

/// A legacy directory detected during migration that may need cleanup.
#[derive(Debug, PartialEq)]
pub struct LegacyDir {
    /// Absolute path to the legacy directory.
    pub path: PathBuf,
    /// Human-readable description of what this directory is.
    pub description: &'static str,
    /// Suggested cleanup command.
    pub cleanup_hint: &'static str,
}

/// Result of a `weave migrate` operation.
#[derive(Debug, PartialEq)]
pub enum MigrateResult {
    /// `weave.lock` already exists — nothing to do.
    AlreadyMigrated,
    /// No legacy `.weave/lock.toml` found and no orphaned artifacts detected.
    NothingToMigrate,
    /// No lockfile to migrate, but orphaned legacy directories were found.
    OrphanedDirs(Vec<LegacyDir>),
    /// Successfully migrated N packages (M new checkouts, S local-source skips).
    Migrated {
        count: usize,
        checkouts: usize,
        local_skipped: usize,
    },
}

/// Detect legacy directories that exist without a corresponding lockfile.
fn detect_legacy_dirs(project_root: &Path) -> Vec<LegacyDir> {
    let mut dirs = Vec::new();

    let weave_deps = project_root.join(".weave").join("deps");
    if weave_deps.is_dir() {
        dirs.push(LegacyDir {
            path: weave_deps,
            description: "orphaned .weave/deps/ (no .weave/lock.toml)",
            cleanup_hint: "rm -rf .weave/deps/",
        });
    }

    let weave_dir = project_root.join(".weave");
    if weave_dir.is_dir() && is_dir_empty_or_only_deps(&weave_dir) {
        // If .weave/ only contained deps (now accounted for above) or is empty,
        // suggest removing the whole directory.
        if !dirs.is_empty() {
            // Replace the deps-specific hint with a whole-directory hint.
            dirs.clear();
            dirs.push(LegacyDir {
                path: weave_dir,
                description: "orphaned .weave/ directory (no lock.toml, only deps/)",
                cleanup_hint: "rm -rf .weave/",
            });
        }
    }

    let csa_patterns = project_root.join(".csa").join("patterns");
    if csa_patterns.is_dir() {
        dirs.push(LegacyDir {
            path: csa_patterns,
            description: "legacy .csa/patterns/ directory (skills now managed via weave.lock)",
            cleanup_hint: "rm -rf .csa/patterns/",
        });
    }

    dirs
}

/// Check whether a directory is empty or only contains a `deps/` subdirectory.
fn is_dir_empty_or_only_deps(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name != "deps" {
            return false;
        }
    }
    true
}

/// Migrate from legacy `.weave/lock.toml` to the new `weave.lock` format,
/// ensuring all git-sourced packages are checked out in the global store.
///
/// Returns the migration outcome. When no lockfile is found, checks for
/// orphaned legacy directories and returns actionable suggestions.
pub fn migrate(project_root: &Path, cache_root: &Path, store_root: &Path) -> Result<MigrateResult> {
    let new_path = lockfile_path(project_root);
    if new_path.is_file() {
        return Ok(MigrateResult::AlreadyMigrated);
    }

    let old_path = legacy_lockfile_path(project_root);
    if !old_path.is_file() {
        let legacy_dirs = detect_legacy_dirs(project_root);
        if !legacy_dirs.is_empty() {
            return Ok(MigrateResult::OrphanedDirs(legacy_dirs));
        }
        return Ok(MigrateResult::NothingToMigrate);
    }

    let lockfile = load_lockfile(&old_path)
        .with_context(|| format!("failed to read legacy lockfile at {}", old_path.display()))?;

    let mut migrated_count: usize = 0;
    let mut local_skipped: usize = 0;

    for pkg in &lockfile.package {
        if pkg.source_kind != SourceKind::Git {
            local_skipped += 1;
            continue;
        }
        if pkg.repo.is_empty() || pkg.commit.is_empty() {
            continue;
        }

        let dest = package_dir(store_root, &pkg.name, &pkg.commit)?;
        if is_checkout_valid(&dest) {
            continue;
        }

        let cas = ensure_cached(cache_root, &pkg.repo)?;
        checkout_to(&cas, &pkg.commit, &dest)?;
        migrated_count += 1;
    }

    save_lockfile(&new_path, &lockfile)?;

    Ok(MigrateResult::Migrated {
        count: lockfile.package.len(),
        checkouts: migrated_count,
        local_skipped,
    })
}
