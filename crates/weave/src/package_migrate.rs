//! Migrate from legacy `.weave/lock.toml` to `weave.lock` and global store.
//!
//! Split from `package.rs` to stay under the monolith-file limit.

use std::path::Path;

use anyhow::{Context, Result};

use super::{
    SourceKind, checkout_to, ensure_cached, is_checkout_valid, legacy_lockfile_path, load_lockfile,
    lockfile_path, package_dir, save_lockfile,
};

/// Result of a `weave migrate` operation.
#[derive(Debug, PartialEq)]
pub enum MigrateResult {
    /// `weave.lock` already exists — nothing to do.
    AlreadyMigrated,
    /// No legacy `.weave/lock.toml` found.
    NothingToMigrate,
    /// Successfully migrated N packages (M new checkouts, S local-source skips).
    Migrated {
        count: usize,
        checkouts: usize,
        local_skipped: usize,
    },
}

/// Migrate from legacy `.weave/lock.toml` to the new `weave.lock` format,
/// ensuring all git-sourced packages are checked out in the global store.
///
/// Returns the migration outcome. Prints nothing to migrate if the
/// legacy lockfile does not exist or the new lockfile already exists.
pub fn migrate(project_root: &Path, cache_root: &Path, store_root: &Path) -> Result<MigrateResult> {
    let new_path = lockfile_path(project_root);
    if new_path.is_file() {
        return Ok(MigrateResult::AlreadyMigrated);
    }

    let old_path = legacy_lockfile_path(project_root);
    if !old_path.is_file() {
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
