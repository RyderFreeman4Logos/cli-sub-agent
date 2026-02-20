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

// ---------------------------------------------------------------------------
// Built-in migrations
// ---------------------------------------------------------------------------

/// Build a registry pre-loaded with all known migrations.
pub fn default_registry() -> MigrationRegistry {
    let mut r = MigrationRegistry::new();
    r.register(plan_to_workflow_migration());
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
