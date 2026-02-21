//! Migration framework for CSA project config and state evolution.
//!
//! Migrations are versioned, ordered, and idempotent. Each migration
//! transforms project state from one version to another via a series
//! of declarative steps (rename files, replace text) or custom logic.

use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

/// Custom migration function signature.
pub type MigrateFn = Box<dyn Fn(&Path) -> Result<()> + Send + Sync>;

/// Semantic version for migration ordering.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::str::FromStr for Version {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            anyhow::bail!("invalid version format: expected MAJOR.MINOR.PATCH, got {s:?}");
        }
        Ok(Self {
            major: parts[0].parse()?,
            minor: parts[1].parse()?,
            patch: parts[2].parse()?,
        })
    }
}

/// A single migration that transforms project state between versions.
pub struct Migration {
    /// Unique identifier (e.g., "0.12.0-plan-to-workflow").
    pub id: String,
    /// Minimum version this migration applies from.
    pub from_version: Version,
    /// Version after this migration is applied.
    pub to_version: Version,
    /// Human-readable description.
    pub description: String,
    /// Ordered steps to execute.
    pub steps: Vec<MigrationStep>,
}

/// An individual step within a migration.
pub enum MigrationStep {
    /// Rename a file relative to the project root.
    RenameFile { from: PathBuf, to: PathBuf },
    /// Replace all occurrences of a string in a file.
    ReplaceInFile {
        path: PathBuf,
        old: String,
        new: String,
    },
    /// Custom migration logic with a descriptive label.
    Custom { label: String, apply: MigrateFn },
}

impl std::fmt::Debug for MigrationStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RenameFile { from, to } => f
                .debug_struct("RenameFile")
                .field("from", from)
                .field("to", to)
                .finish(),
            Self::ReplaceInFile { path, old, new } => f
                .debug_struct("ReplaceInFile")
                .field("path", path)
                .field("old", old)
                .field("new", new)
                .finish(),
            Self::Custom { label, .. } => f.debug_struct("Custom").field("label", label).finish(),
        }
    }
}

/// Registry holding all known migrations, ordered by version.
pub struct MigrationRegistry {
    migrations: Vec<Migration>,
}

impl MigrationRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            migrations: Vec::new(),
        }
    }

    /// Register a migration. Maintains ordering by `from_version`.
    pub fn register(&mut self, migration: Migration) {
        self.migrations.push(migration);
        self.migrations
            .sort_by(|a, b| a.from_version.cmp(&b.from_version));
    }

    /// List all registered migrations.
    pub fn all(&self) -> &[Migration] {
        &self.migrations
    }

    /// Find migrations that need to run to get from `current` to `target`,
    /// excluding those already applied (by id).
    pub fn pending(
        &self,
        current: &Version,
        target: &Version,
        applied: &[String],
    ) -> Vec<&Migration> {
        self.migrations
            .iter()
            .filter(|m| {
                m.from_version >= *current
                    && m.to_version <= *target
                    && !applied.iter().any(|id| id == &m.id)
            })
            .collect()
    }
}

impl Default for MigrationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

const XDG_MIGRATION_LOCK_FILE: &str = "xdg-paths-migration.lock";
const XDG_MIGRATION_MARKER_FILE: &str = ".migration-in-progress";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct XdgMigrationMarker {
    operations: Vec<XdgMigrationOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum XdgMigrationOperation {
    MoveLegacyToNew { legacy: PathBuf, new_path: PathBuf },
    BackupLegacy { legacy: PathBuf, backup: PathBuf },
    CreateLegacySymlink { legacy: PathBuf },
}

#[derive(Debug)]
struct GlobalMigrationLock {
    _file: File,
}

impl GlobalMigrationLock {
    fn acquire(lock_dir: &Path) -> Result<Self> {
        fs::create_dir_all(lock_dir).with_context(|| {
            format!("failed to create migration lock dir {}", lock_dir.display())
        })?;
        let lock_path = lock_dir.join(XDG_MIGRATION_LOCK_FILE);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open migration lock {}", lock_path.display()))?;

        #[cfg(unix)]
        {
            let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if ret != 0 {
                let err = std::io::Error::last_os_error();
                anyhow::bail!(
                    "failed to acquire migration lock {}: {}",
                    lock_path.display(),
                    err
                );
            }
        }

        Ok(Self { _file: file })
    }
}

fn migration_admin_dir() -> PathBuf {
    std::env::temp_dir().join(format!("{}-migrate", paths::APP_NAME))
}

fn marker_path(admin_dir: &Path) -> PathBuf {
    admin_dir.join(XDG_MIGRATION_MARKER_FILE)
}

fn read_marker(path: &Path) -> Result<XdgMigrationMarker> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read migration marker {}", path.display()))?;
    let marker: XdgMigrationMarker = toml::from_str(&raw)
        .with_context(|| format!("failed to parse migration marker {}", path.display()))?;
    Ok(marker)
}

fn write_marker(path: &Path, marker: &XdgMigrationMarker) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create marker parent {}", parent.display()))?;
    }
    let raw = toml::to_string_pretty(marker).context("failed to serialize migration marker")?;
    fs::write(path, raw).with_context(|| format!("failed to write marker {}", path.display()))?;
    Ok(())
}

fn append_marker_operation(
    path: &Path,
    marker: &mut XdgMigrationMarker,
    op: XdgMigrationOperation,
) -> Result<()> {
    marker.operations.push(op);
    write_marker(path, marker)
}

fn rollback_from_marker(marker: &XdgMigrationMarker) -> Result<()> {
    for op in marker.operations.iter().rev() {
        match op {
            XdgMigrationOperation::CreateLegacySymlink { legacy } => {
                if let Ok(meta) = fs::symlink_metadata(legacy)
                    && meta.file_type().is_symlink()
                {
                    fs::remove_file(legacy).with_context(|| {
                        format!(
                            "failed to remove legacy symlink during rollback: {}",
                            legacy.display()
                        )
                    })?;
                }
            }
            XdgMigrationOperation::MoveLegacyToNew { legacy, new_path } => {
                if !legacy.exists() && new_path.exists() {
                    if let Some(parent) = legacy.parent() {
                        fs::create_dir_all(parent).with_context(|| {
                            format!("failed to create rollback parent {}", parent.display())
                        })?;
                    }
                    fs::rename(new_path, legacy).with_context(|| {
                        format!(
                            "failed to rollback renamed path {} -> {}",
                            new_path.display(),
                            legacy.display()
                        )
                    })?;
                }
            }
            XdgMigrationOperation::BackupLegacy { legacy, backup } => {
                if !legacy.exists() && backup.exists() {
                    fs::rename(backup, legacy).with_context(|| {
                        format!(
                            "failed to rollback backup {} -> {}",
                            backup.display(),
                            legacy.display()
                        )
                    })?;
                }
            }
        }
    }
    Ok(())
}

fn recover_incomplete_xdg_migration(admin_dir: &Path) -> Result<()> {
    let path = marker_path(admin_dir);
    if !path.exists() {
        return Ok(());
    }
    let marker = read_marker(&path)?;
    rollback_from_marker(&marker)?;
    fs::remove_file(&path)
        .with_context(|| format!("failed to remove migration marker {}", path.display()))?;
    Ok(())
}

fn unique_backup_path(legacy: &Path) -> PathBuf {
    let mut index = 0_u32;
    loop {
        let suffix = if index == 0 {
            ".migration-backup".to_string()
        } else {
            format!(".migration-backup-{index}")
        };
        let candidate = legacy.with_extension(
            legacy
                .extension()
                .map(|ext| format!("{}{}", ext.to_string_lossy(), suffix))
                .unwrap_or_else(|| suffix.trim_start_matches('.').to_string()),
        );
        if !candidate.exists() {
            return candidate;
        }
        index = index.saturating_add(1);
    }
}

fn is_directory_empty(path: &Path) -> Result<bool> {
    let mut entries = fs::read_dir(path)
        .with_context(|| format!("failed to read directory {}", path.display()))?;
    Ok(entries.next().is_none())
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(src)
        .with_context(|| format!("failed to stat source path {}", src.display()))?;
    if metadata.is_dir() {
        fs::create_dir_all(dst)
            .with_context(|| format!("failed to create destination dir {}", dst.display()))?;
        for entry in fs::read_dir(src)
            .with_context(|| format!("failed to read source dir {}", src.display()))?
        {
            let entry =
                entry.with_context(|| format!("failed to read entry in {}", src.display()))?;
            let name = entry.file_name();
            copy_tree(&entry.path(), &dst.join(name))?;
        }
    } else {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create destination parent {}", parent.display())
            })?;
        }
        fs::copy(src, dst).with_context(|| {
            format!(
                "failed to copy legacy path {} into {}",
                src.display(),
                dst.display()
            )
        })?;
    }
    Ok(())
}

fn merge_legacy_into_new(legacy: &Path, new_path: &Path) -> Result<()> {
    if !legacy.exists() {
        return Ok(());
    }
    if !new_path.exists() {
        fs::create_dir_all(new_path)
            .with_context(|| format!("failed to create merge target {}", new_path.display()))?;
    }

    if legacy.is_dir() && new_path.is_dir() {
        for entry in fs::read_dir(legacy)
            .with_context(|| format!("failed to read legacy dir {}", legacy.display()))?
        {
            let entry =
                entry.with_context(|| format!("failed to read entry in {}", legacy.display()))?;
            let src = entry.path();
            let dst = new_path.join(entry.file_name());
            if !dst.exists() {
                copy_tree(&src, &dst)?;
                continue;
            }
            if src.is_dir() && dst.is_dir() {
                merge_legacy_into_new(&src, &dst)?;
                continue;
            }

            // Conflict: preserve canonical target and keep a copy of legacy data.
            let mut conflict_index = 0_u32;
            loop {
                let suffix = if conflict_index == 0 {
                    ".legacy".to_string()
                } else {
                    format!(".legacy-{conflict_index}")
                };
                let file_name = entry.file_name().to_string_lossy().to_string();
                let conflict_target = new_path.join(format!("{file_name}{suffix}"));
                if !conflict_target.exists() {
                    copy_tree(&src, &conflict_target)?;
                    break;
                }
                conflict_index = conflict_index.saturating_add(1);
            }
        }
        return Ok(());
    }

    if !new_path.exists() {
        copy_tree(legacy, new_path)?;
    }
    Ok(())
}

fn create_legacy_symlink(new_path: &Path, legacy_path: &Path) -> Result<()> {
    if let Some(parent) = legacy_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create legacy parent {}", parent.display()))?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(new_path, legacy_path).with_context(|| {
            format!(
                "failed to create legacy symlink {} -> {}",
                legacy_path.display(),
                new_path.display()
            )
        })?;
    }

    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(new_path, legacy_path).with_context(|| {
            format!(
                "failed to create legacy symlink {} -> {}",
                legacy_path.display(),
                new_path.display()
            )
        })?;
    }

    #[cfg(not(any(unix, windows)))]
    {
        anyhow::bail!(
            "legacy symlink not supported on this platform: {} -> {}",
            legacy_path.display(),
            new_path.display()
        );
    }

    Ok(())
}

fn cleanup_backup_paths(marker: &XdgMigrationMarker) {
    for op in &marker.operations {
        let XdgMigrationOperation::BackupLegacy { backup, .. } = op else {
            continue;
        };
        if backup.is_dir() {
            let _ = fs::remove_dir_all(backup);
        } else if backup.exists() {
            let _ = fs::remove_file(backup);
        }
    }
}

fn migrate_xdg_paths_for_pairs(pairs: Vec<paths::XdgPathPair>, admin_dir: &Path) -> Result<()> {
    let _lock = GlobalMigrationLock::acquire(admin_dir)?;

    recover_incomplete_xdg_migration(admin_dir)?;

    if !pairs.iter().any(|pair| pair.legacy_path.exists()) {
        return Ok(());
    }

    let marker_path = marker_path(admin_dir);
    let mut marker = XdgMigrationMarker::default();
    write_marker(&marker_path, &marker)?;

    let migration_result: Result<()> = (|| {
        for pair in pairs {
            if !pair.legacy_path.exists() {
                continue;
            }

            if pair.new_path.exists() && pair.legacy_path == pair.new_path {
                continue;
            }

            if pair.new_path.exists() {
                let should_merge = if pair.legacy_path.is_dir() {
                    !is_directory_empty(&pair.legacy_path)?
                } else {
                    true
                };
                if should_merge {
                    merge_legacy_into_new(&pair.legacy_path, &pair.new_path)?;
                }

                let backup_path = unique_backup_path(&pair.legacy_path);
                fs::rename(&pair.legacy_path, &backup_path).with_context(|| {
                    format!(
                        "failed to back up legacy {} to {}",
                        pair.legacy_path.display(),
                        backup_path.display()
                    )
                })?;
                append_marker_operation(
                    &marker_path,
                    &mut marker,
                    XdgMigrationOperation::BackupLegacy {
                        legacy: pair.legacy_path.clone(),
                        backup: backup_path,
                    },
                )?;
            } else {
                if let Some(parent) = pair.new_path.parent() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create new path parent {}", parent.display())
                    })?;
                }
                fs::rename(&pair.legacy_path, &pair.new_path).with_context(|| {
                    format!(
                        "failed to move legacy {} to {}",
                        pair.legacy_path.display(),
                        pair.new_path.display()
                    )
                })?;
                append_marker_operation(
                    &marker_path,
                    &mut marker,
                    XdgMigrationOperation::MoveLegacyToNew {
                        legacy: pair.legacy_path.clone(),
                        new_path: pair.new_path.clone(),
                    },
                )?;
            }

            create_legacy_symlink(&pair.new_path, &pair.legacy_path)?;
            append_marker_operation(
                &marker_path,
                &mut marker,
                XdgMigrationOperation::CreateLegacySymlink {
                    legacy: pair.legacy_path,
                },
            )?;
        }
        Ok(())
    })();

    if let Err(error) = migration_result {
        let rollback_result = rollback_from_marker(&marker);
        let remove_marker_result = fs::remove_file(&marker_path);
        if let Err(rollback_error) = rollback_result {
            return Err(error.context(format!("rollback failed: {rollback_error:#}")));
        }
        if let Err(remove_error) = remove_marker_result
            && remove_error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error.context(format!(
                "failed to remove migration marker {}: {}",
                marker_path.display(),
                remove_error
            )));
        }
        return Err(error);
    }

    cleanup_backup_paths(&marker);
    fs::remove_file(&marker_path).with_context(|| {
        format!(
            "failed to remove migration marker {}",
            marker_path.display()
        )
    })?;
    Ok(())
}

fn migrate_xdg_paths(_project_root: &Path) -> Result<()> {
    migrate_xdg_paths_for_pairs(paths::xdg_path_pairs(), &migration_admin_dir())
}

/// Execute a single migration step against a project root.
pub fn execute_step(step: &MigrationStep, project_root: &Path) -> Result<()> {
    match step {
        MigrationStep::RenameFile { from, to } => {
            let src = project_root.join(from);
            let dst = project_root.join(to);
            if src.exists() {
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::rename(&src, &dst)?;
            }
            // Idempotent: if src doesn't exist, assume already renamed.
            Ok(())
        }
        MigrationStep::ReplaceInFile { path, old, new } => {
            let full_path = project_root.join(path);
            if full_path.exists() {
                let content = std::fs::read_to_string(&full_path)?;
                let replaced = content.replace(old.as_str(), new.as_str());
                if replaced != content {
                    std::fs::write(&full_path, replaced)?;
                }
            }
            // Idempotent: missing file or no matches = no-op.
            Ok(())
        }
        MigrationStep::Custom { apply, .. } => apply(project_root),
    }
}

/// Execute all steps in a migration, returning early on first error.
pub fn execute_migration(migration: &Migration, project_root: &Path) -> Result<()> {
    for step in &migration.steps {
        execute_step(step, project_root)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Built-in migrations
// ---------------------------------------------------------------------------

/// Build a registry pre-loaded with all known migrations.
pub fn default_registry() -> MigrationRegistry {
    let mut r = MigrationRegistry::new();
    r.register(plan_to_workflow_migration());
    r.register(xdg_paths_unification_migration());
    r
}

/// Migration 0.1.2: rename `[plan]` → `[workflow]` in workflow TOML files.
///
/// Applies to all `workflow.toml` files under `patterns/` and any
/// `.csa/config.toml` references. The weave compiler already accepts
/// both keys via `#[serde(alias = "plan")]`, so this is a forward-only
/// rename to the canonical key name.
fn plan_to_workflow_migration() -> Migration {
    Migration {
        id: "0.1.2-plan-to-workflow".to_string(),
        from_version: Version::new(0, 1, 1),
        to_version: Version::new(0, 1, 2),
        description: "Rename [plan] to [workflow] in workflow TOML files".to_string(),
        steps: vec![MigrationStep::Custom {
            label: "rename plan keys to workflow in all workflow.toml files".to_string(),
            apply: Box::new(rename_plan_keys_in_project),
        }],
    }
}

fn xdg_paths_unification_migration() -> Migration {
    Migration {
        id: "0.1.27-xdg-paths-unification".to_string(),
        from_version: Version::new(0, 1, 27),
        to_version: Version::new(0, 1, 27),
        description: "Unify XDG paths under cli-sub-agent and keep legacy symlink compatibility"
            .to_string(),
        steps: vec![MigrationStep::Custom {
            label: "migrate xdg paths from csa to cli-sub-agent".to_string(),
            apply: Box::new(migrate_xdg_paths),
        }],
    }
}

/// Walk `patterns/` looking for `workflow.toml` files and replace
/// `[plan]` / `[[plan.` / `plan.` table references with `workflow`.
fn rename_plan_keys_in_project(project_root: &Path) -> Result<()> {
    let patterns_dir = project_root.join("patterns");
    if !patterns_dir.is_dir() {
        return Ok(());
    }
    rename_plan_keys_recursive(&patterns_dir)
}

fn rename_plan_keys_recursive(dir: &Path) -> Result<()> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            rename_plan_keys_recursive(&path)?;
        } else if path.file_name().and_then(|n| n.to_str()) == Some("workflow.toml") {
            rename_plan_keys_in_file(&path)?;
        }
    }
    Ok(())
}

/// Replace plan-based TOML keys with workflow-based keys in a single file.
/// Handles: `[plan]`, `[[plan.steps]]`, `[[plan.variables]]`, `[plan.steps.on_fail]`,
/// `[plan.steps.loop_var]` and similar nested table paths.
fn rename_plan_keys_in_file(path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let mut result = String::with_capacity(content.len());

    for line in content.lines() {
        let trimmed = line.trim();
        if is_plan_table_header(trimmed) {
            // Replace `plan` with `workflow` only in the table key portion.
            let replaced = replace_plan_in_header(line);
            result.push_str(&replaced);
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }

    // Only write if actually changed.
    if result != content {
        std::fs::write(path, result)?;
    }
    Ok(())
}

/// Check if a trimmed line is a TOML table header referencing `plan`.
fn is_plan_table_header(trimmed: &str) -> bool {
    // Matches `[plan]`, `[[plan.steps]]`, `[plan.steps.on_fail]`, etc.
    (trimmed.starts_with('[') && trimmed.ends_with(']'))
        && (trimmed.contains("[plan]") || trimmed.contains("[plan.") || trimmed.contains("[[plan."))
}

/// Replace `plan` with `workflow` in a TOML table header line,
/// preserving leading whitespace and bracket structure.
fn replace_plan_in_header(line: &str) -> String {
    // We only replace the first occurrence of `plan` that appears
    // right after `[` or `[[` — this avoids false positives.
    line.replacen("[plan]", "[workflow]", 1)
        .replacen("[[plan.", "[[workflow.", 1)
        .replacen("[plan.", "[workflow.", 1)
}

#[cfg(test)]
#[path = "migrate_tests.rs"]
mod tests;
