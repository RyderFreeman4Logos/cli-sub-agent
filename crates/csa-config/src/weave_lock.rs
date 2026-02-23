//! Read/write `weave.lock` — version and migration tracking for CSA projects.
//!
//! The lock file lives at `{project_root}/weave.lock` and records:
//! - Current csa/weave versions
//! - Which migrations have been applied
//! - Timestamp of the last migration run

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const LOCK_FILENAME: &str = "weave.lock";

/// Top-level weave.lock structure.
///
/// The lock file may contain both CSA version/migration tracking (`[versions]`,
/// `[migrations]`) and weave package entries (`[[package]]`). Each section is
/// optional so that the parser can read files written by either subsystem
/// without "missing field" errors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WeaveLock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versions: Option<LockVersions>,
    #[serde(default)]
    pub migrations: LockMigrations,
    /// Weave package entries — preserved across load/save so that CSA version
    /// updates do not discard package data written by `weave install`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package: Vec<toml::Value>,
}

/// Version snapshot recorded in the lock file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockVersions {
    pub csa: String,
    pub weave: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_migrated_at: Option<DateTime<Utc>>,
}

/// Migration tracking state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LockMigrations {
    #[serde(default)]
    pub applied: Vec<String>,
}

impl WeaveLock {
    /// Load from `{project_dir}/weave.lock`.
    /// Returns `None` if the file does not exist.
    pub fn load(project_dir: &Path) -> Result<Option<Self>> {
        let path = lock_path(project_dir);
        if !path.exists() {
            return Ok(None);
        }
        let content =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let lock: Self =
            toml::from_str(&content).with_context(|| format!("parsing {}", path.display()))?;
        Ok(Some(lock))
    }

    /// Load from `{project_dir}/weave.lock`, creating a default if missing.
    pub fn load_or_init(
        project_dir: &Path,
        csa_version: &str,
        weave_version: &str,
    ) -> Result<Self> {
        match Self::load(project_dir)? {
            Some(lock) => Ok(lock),
            None => {
                let lock = Self::new(csa_version, weave_version);
                lock.save(project_dir)?;
                Ok(lock)
            }
        }
    }

    /// Create a fresh lock with current versions and no applied migrations.
    pub fn new(csa_version: &str, weave_version: &str) -> Self {
        Self {
            versions: Some(LockVersions {
                csa: csa_version.to_string(),
                weave: weave_version.to_string(),
                last_migrated_at: None,
            }),
            migrations: LockMigrations::default(),
            package: Vec::new(),
        }
    }

    /// Returns the versions section, if present.
    pub fn versions(&self) -> Option<&LockVersions> {
        self.versions.as_ref()
    }

    /// Returns a mutable reference to the versions section, if present.
    pub fn versions_mut(&mut self) -> Option<&mut LockVersions> {
        self.versions.as_mut()
    }

    /// Ensures a versions section exists, creating one with the given values
    /// if absent.
    pub fn versions_or_init(
        &mut self,
        csa_version: &str,
        weave_version: &str,
    ) -> &mut LockVersions {
        self.versions.get_or_insert_with(|| LockVersions {
            csa: csa_version.to_string(),
            weave: weave_version.to_string(),
            last_migrated_at: None,
        })
    }

    /// Write atomically to `{project_dir}/weave.lock`.
    pub fn save(&self, project_dir: &Path) -> Result<()> {
        let path = lock_path(project_dir);
        let content = toml::to_string_pretty(self).context("serializing weave.lock")?;

        // Atomic write: write to temp file then rename.
        let tmp_path = path.with_extension("lock.tmp");
        fs::write(&tmp_path, content.as_bytes())
            .with_context(|| format!("writing {}", tmp_path.display()))?;
        fs::rename(&tmp_path, &path)
            .with_context(|| format!("renaming {} to {}", tmp_path.display(), path.display()))?;
        Ok(())
    }

    /// Record a migration as applied.
    pub fn record_migration(&mut self, migration_id: &str) {
        if !self.migrations.applied.contains(&migration_id.to_string()) {
            self.migrations.applied.push(migration_id.to_string());
        }
        if let Some(v) = self.versions.as_mut() {
            v.last_migrated_at = Some(Utc::now());
        }
    }

    /// Check whether a migration has already been applied.
    pub fn is_migration_applied(&self, migration_id: &str) -> bool {
        self.migrations.applied.iter().any(|id| id == migration_id)
    }
}

/// Result of comparing the running binary version against weave.lock.
pub enum VersionCheckResult {
    /// No weave.lock exists yet; nothing to do.
    NoLockFile,
    /// Versions match — everything is current.
    UpToDate,
    /// Version changed but no migrations are pending.
    /// The lock file has already been silently updated.
    AutoUpdated,
    /// Migrations are pending; user should run `csa migrate`.
    MigrationNeeded { pending_count: usize },
}

/// Compare the running binary version against `weave.lock` and take
/// the appropriate action:
///
/// - No lock file → return `NoLockFile` (caller may create one).
/// - Versions match → return `UpToDate`.
/// - Version differs, no pending migrations → silently update lock, return `AutoUpdated`.
/// - Version differs, pending migrations → return `MigrationNeeded`.
pub fn check_version(
    project_dir: &Path,
    binary_csa_version: &str,
    binary_weave_version: &str,
    registry: &crate::MigrationRegistry,
) -> Result<VersionCheckResult> {
    let Some(mut lock) = WeaveLock::load(project_dir)? else {
        return Ok(VersionCheckResult::NoLockFile);
    };

    // If the lock file exists but has no [versions] section (e.g. a
    // package-only lockfile written by `weave install`), treat it the same
    // as "no lock file" for version-tracking purposes.
    let Some(versions) = lock.versions.as_ref() else {
        return Ok(VersionCheckResult::NoLockFile);
    };

    let lock_csa = &versions.csa;
    if lock_csa == binary_csa_version {
        return Ok(VersionCheckResult::UpToDate);
    }

    // Parse versions to check for pending migrations.
    let current: crate::Version = lock_csa
        .parse()
        .with_context(|| format!("parsing lock csa version {lock_csa:?}"))?;
    let target: crate::Version = binary_csa_version
        .parse()
        .with_context(|| format!("parsing binary csa version {binary_csa_version:?}"))?;

    let pending = registry.pending(&current, &target, &lock.migrations.applied);

    if pending.is_empty() {
        // No migrations needed — just a patch bump. Auto-update the lock.
        let v = lock.versions_or_init(binary_csa_version, binary_weave_version);
        v.csa = binary_csa_version.to_string();
        v.weave = binary_weave_version.to_string();
        lock.save(project_dir)?;
        return Ok(VersionCheckResult::AutoUpdated);
    }

    Ok(VersionCheckResult::MigrationNeeded {
        pending_count: pending.len(),
    })
}

fn lock_path(project_dir: &Path) -> PathBuf {
    project_dir.join(LOCK_FILENAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_roundtrip_serialization() {
        let lock = WeaveLock::new("0.12.1", "0.8.3");
        let toml_str = toml::to_string_pretty(&lock).unwrap();
        let parsed: WeaveLock = toml::from_str(&toml_str).unwrap();
        assert_eq!(lock, parsed);
    }

    #[test]
    fn test_load_missing_returns_none() {
        let dir = TempDir::new().unwrap();
        let result = WeaveLock::load(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_save_and_load() {
        let dir = TempDir::new().unwrap();
        let lock = WeaveLock::new("0.12.1", "0.8.3");
        lock.save(dir.path()).unwrap();

        let loaded = WeaveLock::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.versions().unwrap().csa, "0.12.1");
        assert_eq!(loaded.versions().unwrap().weave, "0.8.3");
        assert!(loaded.migrations.applied.is_empty());
    }

    #[test]
    fn test_load_or_init_creates_file() {
        let dir = TempDir::new().unwrap();
        let lock = WeaveLock::load_or_init(dir.path(), "1.0.0", "2.0.0").unwrap();
        assert_eq!(lock.versions().unwrap().csa, "1.0.0");

        // File should now exist
        assert!(dir.path().join("weave.lock").exists());
    }

    #[test]
    fn test_record_migration() {
        let mut lock = WeaveLock::new("0.12.0", "0.8.0");
        assert!(!lock.is_migration_applied("0.12.0-plan-to-workflow"));

        lock.record_migration("0.12.0-plan-to-workflow");
        assert!(lock.is_migration_applied("0.12.0-plan-to-workflow"));
        assert!(lock.versions().unwrap().last_migrated_at.is_some());

        // Duplicate should not add twice
        lock.record_migration("0.12.0-plan-to-workflow");
        assert_eq!(lock.migrations.applied.len(), 1);
    }

    #[test]
    fn test_roundtrip_with_migrations() {
        let dir = TempDir::new().unwrap();
        let mut lock = WeaveLock::new("0.12.1", "0.8.3");
        lock.record_migration("0.12.0-plan-to-workflow");
        lock.record_migration("0.12.1-rename-config");
        lock.save(dir.path()).unwrap();

        let loaded = WeaveLock::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.migrations.applied.len(), 2);
        assert!(loaded.is_migration_applied("0.12.0-plan-to-workflow"));
        assert!(loaded.is_migration_applied("0.12.1-rename-config"));
    }

    #[test]
    fn test_check_version_no_lock_file() {
        let dir = TempDir::new().unwrap();
        let registry = crate::MigrationRegistry::new();
        let result = check_version(dir.path(), "0.2.0", "0.2.0", &registry).unwrap();
        assert!(matches!(result, VersionCheckResult::NoLockFile));
    }

    #[test]
    fn test_check_version_up_to_date() {
        let dir = TempDir::new().unwrap();
        let lock = WeaveLock::new("0.1.0", "0.1.0");
        lock.save(dir.path()).unwrap();

        let registry = crate::MigrationRegistry::new();
        let result = check_version(dir.path(), "0.1.0", "0.1.0", &registry).unwrap();
        assert!(matches!(result, VersionCheckResult::UpToDate));
    }

    #[test]
    fn test_check_version_auto_updates_when_no_migrations() {
        let dir = TempDir::new().unwrap();
        let lock = WeaveLock::new("0.1.0", "0.1.0");
        lock.save(dir.path()).unwrap();

        let registry = crate::MigrationRegistry::new();
        let result = check_version(dir.path(), "0.1.1", "0.1.1", &registry).unwrap();
        assert!(matches!(result, VersionCheckResult::AutoUpdated));

        // Verify lock was actually updated.
        let loaded = WeaveLock::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.versions().unwrap().csa, "0.1.1");
    }

    #[test]
    fn test_check_version_migration_needed() {
        let dir = TempDir::new().unwrap();
        let lock = WeaveLock::new("0.1.0", "0.1.0");
        lock.save(dir.path()).unwrap();

        let mut registry = crate::MigrationRegistry::new();
        registry.register(crate::Migration {
            id: "0.1.0-test-migration".to_string(),
            from_version: crate::Version::new(0, 1, 0),
            to_version: crate::Version::new(0, 2, 0),
            description: "Test migration".to_string(),
            steps: vec![],
        });

        let result = check_version(dir.path(), "0.2.0", "0.2.0", &registry).unwrap();
        assert!(matches!(
            result,
            VersionCheckResult::MigrationNeeded { pending_count: 1 }
        ));
    }

    #[test]
    fn test_parse_toml_format() {
        let toml_str = r#"
[versions]
csa = "0.12.1"
weave = "0.8.3"
last_migrated_at = "2026-02-20T00:00:00Z"

[migrations]
applied = ["0.12.0-plan-to-workflow"]
"#;
        let lock: WeaveLock = toml::from_str(toml_str).unwrap();
        assert_eq!(lock.versions().unwrap().csa, "0.12.1");
        assert_eq!(lock.versions().unwrap().weave, "0.8.3");
        assert!(lock.versions().unwrap().last_migrated_at.is_some());
        assert_eq!(lock.migrations.applied, vec!["0.12.0-plan-to-workflow"]);
    }

    #[test]
    fn test_parse_package_only_format() {
        // A weave.lock written by `weave install` has only [[package]] entries
        // and no [versions] section. WeaveLock must parse it without error.
        let toml_str = r#"
[[package]]
name = "my-skill"
repo = "https://github.com/org/my-skill.git"
commit = "abc123def456"
"#;
        let lock: WeaveLock = toml::from_str(toml_str).unwrap();
        assert!(lock.versions.is_none());
        assert!(lock.migrations.applied.is_empty());
        assert_eq!(lock.package.len(), 1);
    }

    #[test]
    fn test_parse_mixed_format() {
        // A weave.lock that contains both [versions] and [[package]] sections.
        let toml_str = r#"
[versions]
csa = "0.1.32"
weave = "0.1.32"

[migrations]
applied = []

[[package]]
name = "audit"
repo = "https://github.com/org/audit.git"
commit = "abc123"
"#;
        let lock: WeaveLock = toml::from_str(toml_str).unwrap();
        assert_eq!(lock.versions().unwrap().csa, "0.1.32");
        assert!(lock.migrations.applied.is_empty());
        assert_eq!(lock.package.len(), 1);
    }

    #[test]
    fn test_save_preserves_package_entries() {
        // When WeaveLock saves, it must preserve [[package]] entries.
        let dir = TempDir::new().unwrap();
        let toml_str = r#"
[versions]
csa = "0.1.0"
weave = "0.1.0"

[[package]]
name = "my-skill"
repo = "https://github.com/org/my-skill.git"
commit = "abc123"
"#;
        let lock: WeaveLock = toml::from_str(toml_str).unwrap();
        assert_eq!(lock.package.len(), 1);
        lock.save(dir.path()).unwrap();

        // Re-read and verify package data survives.
        let loaded = WeaveLock::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.versions().unwrap().csa, "0.1.0");
        assert_eq!(loaded.package.len(), 1);
    }

    #[test]
    fn test_check_version_no_versions_section() {
        // When weave.lock exists but has no [versions], treat as NoLockFile.
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("weave.lock"),
            r#"
[[package]]
name = "pkg"
repo = "https://example.com/pkg.git"
commit = "abc"
"#,
        )
        .unwrap();

        let registry = crate::MigrationRegistry::new();
        let result = check_version(dir.path(), "0.2.0", "0.2.0", &registry).unwrap();
        assert!(matches!(result, VersionCheckResult::NoLockFile));
    }
}
