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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WeaveLock {
    pub versions: LockVersions,
    #[serde(default)]
    pub migrations: LockMigrations,
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
            versions: LockVersions {
                csa: csa_version.to_string(),
                weave: weave_version.to_string(),
                last_migrated_at: None,
            },
            migrations: LockMigrations::default(),
        }
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
        self.versions.last_migrated_at = Some(Utc::now());
    }

    /// Check whether a migration has already been applied.
    pub fn is_migration_applied(&self, migration_id: &str) -> bool {
        self.migrations.applied.iter().any(|id| id == migration_id)
    }
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
        assert_eq!(loaded.versions.csa, "0.12.1");
        assert_eq!(loaded.versions.weave, "0.8.3");
        assert!(loaded.migrations.applied.is_empty());
    }

    #[test]
    fn test_load_or_init_creates_file() {
        let dir = TempDir::new().unwrap();
        let lock = WeaveLock::load_or_init(dir.path(), "1.0.0", "2.0.0").unwrap();
        assert_eq!(lock.versions.csa, "1.0.0");

        // File should now exist
        assert!(dir.path().join("weave.lock").exists());
    }

    #[test]
    fn test_record_migration() {
        let mut lock = WeaveLock::new("0.12.0", "0.8.0");
        assert!(!lock.is_migration_applied("0.12.0-plan-to-workflow"));

        lock.record_migration("0.12.0-plan-to-workflow");
        assert!(lock.is_migration_applied("0.12.0-plan-to-workflow"));
        assert!(lock.versions.last_migrated_at.is_some());

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
        assert_eq!(lock.versions.csa, "0.12.1");
        assert_eq!(lock.versions.weave, "0.8.3");
        assert!(lock.versions.last_migrated_at.is_some());
        assert_eq!(lock.migrations.applied, vec!["0.12.0-plan-to-workflow"]);
    }
}
