//! Migration framework for CSA project config and state evolution.
//!
//! Migrations are versioned, ordered, and idempotent. Each migration
//! transforms project state from one version to another via a series
//! of declarative steps (rename files, replace text) or custom logic.

use std::path::{Path, PathBuf};

use anyhow::Result;

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_version_ordering() {
        let v1 = Version::new(0, 12, 0);
        let v2 = Version::new(0, 12, 1);
        let v3 = Version::new(1, 0, 0);
        assert!(v1 < v2);
        assert!(v2 < v3);
    }

    #[test]
    fn test_version_parse_and_display() {
        let v: Version = "1.2.3".parse().unwrap();
        assert_eq!(v, Version::new(1, 2, 3));
        assert_eq!(v.to_string(), "1.2.3");
    }

    #[test]
    fn test_version_parse_invalid() {
        assert!("1.2".parse::<Version>().is_err());
        assert!("abc".parse::<Version>().is_err());
    }

    #[test]
    fn test_registry_pending_filters_applied() {
        let mut registry = MigrationRegistry::new();
        registry.register(Migration {
            id: "0.12.0-rename-plans".to_string(),
            from_version: Version::new(0, 12, 0),
            to_version: Version::new(0, 12, 1),
            description: "Rename plan files".to_string(),
            steps: vec![],
        });
        registry.register(Migration {
            id: "0.12.1-update-config".to_string(),
            from_version: Version::new(0, 12, 1),
            to_version: Version::new(0, 13, 0),
            description: "Update config format".to_string(),
            steps: vec![],
        });

        let current = Version::new(0, 12, 0);
        let target = Version::new(0, 13, 0);

        // Nothing applied → both pending
        let pending = registry.pending(&current, &target, &[]);
        assert_eq!(pending.len(), 2);

        // First applied → only second pending
        let applied = vec!["0.12.0-rename-plans".to_string()];
        let pending = registry.pending(&current, &target, &applied);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "0.12.1-update-config");
    }

    #[test]
    fn test_rename_file_step() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("old.txt"), "content").unwrap();

        let step = MigrationStep::RenameFile {
            from: PathBuf::from("old.txt"),
            to: PathBuf::from("new.txt"),
        };
        execute_step(&step, dir.path()).unwrap();

        assert!(!dir.path().join("old.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            "content"
        );
    }

    #[test]
    fn test_rename_file_idempotent() {
        let dir = TempDir::new().unwrap();
        // Source doesn't exist — should be a no-op
        let step = MigrationStep::RenameFile {
            from: PathBuf::from("missing.txt"),
            to: PathBuf::from("target.txt"),
        };
        execute_step(&step, dir.path()).unwrap();
    }

    #[test]
    fn test_replace_in_file_step() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[plan]\nkey = \"old\"").unwrap();

        let step = MigrationStep::ReplaceInFile {
            path: PathBuf::from("config.toml"),
            old: "[plan]".to_string(),
            new: "[workflow]".to_string(),
        };
        execute_step(&step, dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert_eq!(content, "[workflow]\nkey = \"old\"");
    }

    #[test]
    fn test_replace_in_file_idempotent() {
        let dir = TempDir::new().unwrap();
        // File doesn't exist — no-op
        let step = MigrationStep::ReplaceInFile {
            path: PathBuf::from("missing.toml"),
            old: "old".to_string(),
            new: "new".to_string(),
        };
        execute_step(&step, dir.path()).unwrap();
    }

    #[test]
    fn test_custom_step() {
        let dir = TempDir::new().unwrap();
        let step = MigrationStep::Custom {
            label: "create marker".to_string(),
            apply: Box::new(|root| {
                std::fs::write(root.join("marker.txt"), "migrated")?;
                Ok(())
            }),
        };
        execute_step(&step, dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("marker.txt")).unwrap(),
            "migrated"
        );
    }

    #[test]
    fn test_execute_migration_runs_all_steps() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();

        let migration = Migration {
            id: "test-migration".to_string(),
            from_version: Version::new(0, 1, 0),
            to_version: Version::new(0, 2, 0),
            description: "Test".to_string(),
            steps: vec![
                MigrationStep::RenameFile {
                    from: PathBuf::from("a.txt"),
                    to: PathBuf::from("b.txt"),
                },
                MigrationStep::ReplaceInFile {
                    path: PathBuf::from("b.txt"),
                    old: "hello".to_string(),
                    new: "world".to_string(),
                },
            ],
        };
        execute_migration(&migration, dir.path()).unwrap();

        assert!(!dir.path().join("a.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "world"
        );
    }

    #[test]
    fn test_registry_ordering() {
        let mut registry = MigrationRegistry::new();
        // Insert out of order
        registry.register(Migration {
            id: "second".to_string(),
            from_version: Version::new(0, 2, 0),
            to_version: Version::new(0, 3, 0),
            description: "Second".to_string(),
            steps: vec![],
        });
        registry.register(Migration {
            id: "first".to_string(),
            from_version: Version::new(0, 1, 0),
            to_version: Version::new(0, 2, 0),
            description: "First".to_string(),
            steps: vec![],
        });

        assert_eq!(registry.all()[0].id, "first");
        assert_eq!(registry.all()[1].id, "second");
    }
}
