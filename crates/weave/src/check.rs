//! Symlink health checking for skill directories.
//!
//! Scans tool-specific skill directories (`.claude/skills/`, `.codex/skills/`,
//! etc.) for broken symlinks and optionally removes them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::warn;

use crate::package::AuditIssue;
/// Default directories where weave manages companion skill symlinks.
pub const DEFAULT_LINK_DIRS: &[&str] = &[".claude/skills", ".codex/skills", ".agents/skills"];

/// Default directories to scan for broken symlinks.
pub const DEFAULT_CHECK_DIRS: &[&str] = &[
    ".claude/skills",
    ".codex/skills",
    ".agents/skills",
    ".gemini/skills",
];

const GEMINI_SKILLS_DIR: &str = ".gemini/skills";
const AGENTS_SKILLS_DIR: &str = ".agents/skills";
const PATTERNS_DIR: &str = "patterns";

/// Result of checking a single directory for broken symlinks.
#[derive(Debug)]
pub struct CheckResult {
    /// Directory that was scanned.
    pub dir: PathBuf,
    /// Broken symlinks found.
    pub issues: Vec<AuditIssue>,
    /// Number of symlinks that were removed (when fix=true).
    pub fixed: usize,
    /// Number of symlinks that could not be removed (permission errors, etc.).
    pub fix_failures: usize,
}

/// A Gemini duplicate symlink that was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiCleanupEntry {
    /// Symlink path that was removed.
    pub path: PathBuf,
    /// Original symlink target as stored on disk.
    pub target: PathBuf,
}

/// A Gemini duplicate symlink that could not be removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiCleanupFailure {
    /// Symlink path that could not be removed.
    pub path: PathBuf,
    /// Human-readable failure reason.
    pub error: String,
}

/// A Gemini skill symlink that was migrated to `.agents/skills/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiMigrateEntry {
    /// Original symlink path in `.gemini/skills/`.
    pub gemini_path: PathBuf,
    /// New symlink path in `.agents/skills/`.
    pub agents_path: PathBuf,
    /// Symlink target used for the new link.
    pub target: PathBuf,
}

/// Result of cleaning duplicate Gemini skill symlinks.
#[derive(Debug, Default)]
pub struct GeminiCleanupResult {
    /// Gemini skills directory that was scanned.
    pub dir: PathBuf,
    /// True when the Gemini skills directory did not exist.
    pub missing_dir: bool,
    /// Duplicate symlinks that were removed.
    pub removed: Vec<GeminiCleanupEntry>,
    /// Duplicate symlinks that could not be removed.
    pub remove_failures: Vec<GeminiCleanupFailure>,
    /// Symlinks migrated from `.gemini/skills/` to `.agents/skills/`.
    pub moved: Vec<GeminiMigrateEntry>,
    /// Symlinks that failed to migrate.
    pub move_failures: Vec<GeminiCleanupFailure>,
    /// Number of regular files/directories that were ignored.
    pub skipped_non_symlink: usize,
    /// Number of symlinks whose names had no managed skill counterpart.
    pub skipped_non_duplicate: usize,
    /// Number of symlinks preserved because they point outside weave-managed paths.
    pub skipped_non_weave_target: usize,
}

fn resolve_symlink_target(link_path: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        target.to_path_buf()
    } else {
        link_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(target)
    }
}

fn canonicalize_existing_target(path: &Path) -> std::io::Result<Option<PathBuf>> {
    match path.try_exists() {
        Ok(true) => std::fs::canonicalize(path).map(Some),
        Ok(false) => Ok(None),
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => Err(err),
        Err(_) => Ok(None),
    }
}

/// Scan directories for broken symlinks.
///
/// When `fix` is true, broken symlinks are removed and the count is returned
/// in `CheckResult::fixed`. Only actual symlinks are removed — regular files
/// and directories are never touched.
pub fn check_symlinks(
    project_root: &Path,
    dirs: &[PathBuf],
    fix: bool,
) -> Result<Vec<CheckResult>> {
    let mut results = Vec::new();

    for dir in dirs {
        let abs_dir = if dir.is_absolute() {
            dir.clone()
        } else {
            project_root.join(dir)
        };

        if !abs_dir.is_dir() {
            continue;
        }

        let mut issues = Vec::new();
        let mut fixed = 0;
        let mut fix_failures = 0;

        let entries = std::fs::read_dir(&abs_dir)
            .with_context(|| format!("failed to read {}", abs_dir.display()))?;

        for entry in entries.filter_map(|e| {
            e.map_err(|err| warn!("failed to read directory entry: {err}"))
                .ok()
        }) {
            let path = entry.path();

            // Use symlink_metadata to inspect the link itself, not its target.
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(err) => {
                    warn!("cannot stat {}: {err}", path.display());
                    continue;
                }
            };

            if !meta.file_type().is_symlink() {
                continue;
            }

            let target = match std::fs::read_link(&path) {
                Ok(t) => t,
                Err(err) => {
                    warn!("cannot read symlink {}: {err}", path.display());
                    continue;
                }
            };

            // Resolve relative targets against the symlink's parent directory.
            let resolved = if target.is_absolute() {
                target.clone()
            } else {
                abs_dir.join(&target)
            };

            // Check if target exists.  Use try_exists() to distinguish
            // "not found" from "permission denied".  Only skip on
            // PermissionDenied (target is inaccessible, not necessarily
            // broken).  Other I/O errors (ENOTDIR, EIO, etc.) mean the
            // target path is structurally invalid → treat as broken.
            let target_exists = match resolved.try_exists() {
                Ok(exists) => exists,
                Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                    warn!("cannot check symlink target {}: {err}", resolved.display());
                    continue;
                }
                Err(err) => {
                    warn!(
                        "symlink target unreachable, treating as broken {}: {err}",
                        resolved.display()
                    );
                    false
                }
            };
            if !target_exists {
                issues.push(AuditIssue::BrokenSymlink {
                    path: path.clone(),
                    target: target.clone(),
                });

                if fix {
                    // Only remove the symlink itself, never follow it.
                    if let Ok(m) = std::fs::symlink_metadata(&path)
                        && m.file_type().is_symlink()
                    {
                        // Try remove_file first (works for file symlinks
                        // on all platforms).  Fall back to remove_dir for
                        // Windows directory symlinks/junctions.
                        match std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir(&path)) {
                            Ok(()) => fixed += 1,
                            Err(_) => fix_failures += 1,
                        }
                    }
                }
            }
        }

        if !issues.is_empty() || fixed > 0 || fix_failures > 0 {
            results.push(CheckResult {
                dir: abs_dir,
                issues,
                fixed,
                fix_failures,
            });
        }
    }

    Ok(results)
}

/// Remove duplicate Gemini skill symlinks whose names already exist in `.agents/skills/`.
///
/// A Gemini symlink is removed only when both of these conditions hold:
/// 1. Its file name already exists under `.agents/skills/`.
/// 2. Its resolved target is weave-managed (inside `patterns/` or `.agents/skills/`)
///    or the symlink is broken.
///
/// This catches both historical direct links to source skill directories and
/// the newer indirect links that point at `.agents/skills/`, while preserving
/// user-defined overrides that happen to share the same name.
pub fn clean_gemini_duplicate_symlinks(project_root: &Path) -> Result<GeminiCleanupResult> {
    let gemini_dir = project_root.join(GEMINI_SKILLS_DIR);
    let agents_dir = project_root.join(AGENTS_SKILLS_DIR);
    let patterns_dir = project_root.join(PATTERNS_DIR);
    let canonical_agents_dir = agents_dir.canonicalize().ok();
    let canonical_patterns_dir = patterns_dir.canonicalize().ok();
    let mut result = GeminiCleanupResult {
        dir: gemini_dir.clone(),
        missing_dir: !gemini_dir.is_dir(),
        ..GeminiCleanupResult::default()
    };

    if result.missing_dir {
        return Ok(result);
    }

    let entries = std::fs::read_dir(&gemini_dir)
        .with_context(|| format!("failed to read {}", gemini_dir.display()))?;

    for entry in entries.filter_map(|e| {
        e.map_err(|err| warn!("failed to read directory entry: {err}"))
            .ok()
    }) {
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(err) => {
                warn!("cannot stat {}: {err}", path.display());
                continue;
            }
        };

        if !meta.file_type().is_symlink() {
            result.skipped_non_symlink += 1;
            continue;
        }

        let target = match std::fs::read_link(&path) {
            Ok(target) => target,
            Err(err) => {
                warn!("cannot read symlink {}: {err}", path.display());
                continue;
            }
        };

        let managed_skill = agents_dir.join(entry.file_name());
        let is_duplicate = match std::fs::symlink_metadata(&managed_skill) {
            Ok(_) => true,
            Err(err) => {
                warn!(
                    "cannot check managed skill counterpart {}: {err}",
                    managed_skill.display()
                );
                false
            }
        };
        if !is_duplicate {
            result.skipped_non_duplicate += 1;
            continue;
        }

        let resolved_target = resolve_symlink_target(&path, &target);
        let canonical_target = match canonicalize_existing_target(&resolved_target) {
            Ok(target) => target,
            Err(err) => {
                warn!(
                    "cannot resolve Gemini symlink target {}: {err}",
                    resolved_target.display()
                );
                continue;
            }
        };
        let is_weave_managed = canonical_target.as_ref().is_none_or(|resolved| {
            canonical_patterns_dir
                .as_ref()
                .is_some_and(|patterns| resolved.starts_with(patterns))
                || canonical_agents_dir
                    .as_ref()
                    .is_some_and(|agents| resolved.starts_with(agents))
        });
        if !is_weave_managed {
            let non_weave_target = canonical_target.as_ref().unwrap_or(&resolved_target);
            warn!(
                "Skipping {}: points to non-weave target {}",
                path.display(),
                non_weave_target.display()
            );
            result.skipped_non_weave_target += 1;
            continue;
        }

        match std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir(&path)) {
            Ok(()) => result.removed.push(GeminiCleanupEntry { path, target }),
            Err(err) => result.remove_failures.push(GeminiCleanupFailure {
                path,
                error: err.to_string(),
            }),
        }
    }

    Ok(result)
}
/// Migrate all weave-managed symlinks from `.gemini/skills/` to `.agents/skills/`.
///
/// For each symlink in `.gemini/skills/`:
/// - If its name already exists in `.agents/skills/` (duplicate): remove from `.gemini/skills/`.
/// - If its name does NOT exist in `.agents/skills/`: recreate the symlink in `.agents/skills/`
///   with a recalculated relative path, then remove from `.gemini/skills/`.
/// - Non-symlinks and symlinks pointing outside weave-managed paths are preserved.
///
/// This forms a union of both directories, eliminating duplicates that cause conflicts
/// when Gemini detects both `.gemini/skills/` and `.agents/skills/`.
pub fn migrate_gemini_skills(project_root: &Path) -> Result<GeminiCleanupResult> {
    let gemini_dir = project_root.join(GEMINI_SKILLS_DIR);
    let agents_dir = project_root.join(AGENTS_SKILLS_DIR);
    let patterns_dir = project_root.join(PATTERNS_DIR);
    let canonical_agents_dir = agents_dir.canonicalize().ok();
    let canonical_patterns_dir = patterns_dir.canonicalize().ok();
    let mut result = GeminiCleanupResult {
        dir: gemini_dir.clone(),
        missing_dir: !gemini_dir.is_dir(),
        ..GeminiCleanupResult::default()
    };

    if result.missing_dir {
        return Ok(result);
    }

    let entries = std::fs::read_dir(&gemini_dir)
        .with_context(|| format!("failed to read {}", gemini_dir.display()))?;

    for entry in entries.filter_map(|e| {
        e.map_err(|err| warn!("failed to read directory entry: {err}"))
            .ok()
    }) {
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(err) => {
                warn!("cannot stat {}: {err}", path.display());
                continue;
            }
        };

        if !meta.file_type().is_symlink() {
            result.skipped_non_symlink += 1;
            continue;
        }

        let target = match std::fs::read_link(&path) {
            Ok(target) => target,
            Err(err) => {
                warn!("cannot read symlink {}: {err}", path.display());
                continue;
            }
        };

        // Determine if the symlink is weave-managed (target in patterns/ or .agents/skills/)
        // or broken (target doesn't exist — likely stale weave link).
        let resolved_target = resolve_symlink_target(&path, &target);
        let canonical_target = match canonicalize_existing_target(&resolved_target) {
            Ok(target) => target,
            Err(err) => {
                warn!(
                    "cannot resolve Gemini symlink target {}: {err}",
                    resolved_target.display()
                );
                continue;
            }
        };
        let is_weave_managed = canonical_target.as_ref().is_none_or(|resolved| {
            canonical_patterns_dir
                .as_ref()
                .is_some_and(|patterns| resolved.starts_with(patterns))
                || canonical_agents_dir
                    .as_ref()
                    .is_some_and(|agents| resolved.starts_with(agents))
        });

        if !is_weave_managed {
            let non_weave_target = canonical_target.as_ref().unwrap_or(&resolved_target);
            warn!(
                "Skipping {}: points to non-weave target {}",
                path.display(),
                non_weave_target.display()
            );
            result.skipped_non_weave_target += 1;
            continue;
        }

        let managed_skill = agents_dir.join(entry.file_name());
        let is_duplicate = std::fs::symlink_metadata(&managed_skill).is_ok();

        if is_duplicate {
            // Same name already exists in .agents/skills/ — just remove from .gemini/skills/.
            match crate::link::remove_symlink(&path) {
                Ok(()) => result.removed.push(GeminiCleanupEntry { path, target }),
                Err(err) => result.remove_failures.push(GeminiCleanupFailure {
                    path,
                    error: err.to_string(),
                }),
            }
        } else {
            // Not in .agents/skills/ yet — migrate by creating a new symlink there.
            if !agents_dir.exists() {
                std::fs::create_dir_all(&agents_dir)
                    .with_context(|| format!("cannot create {}", agents_dir.display()))?;
            }

            // Recompute the relative target from .agents/skills/ to the actual skill source.
            let abs_target = canonical_target
                .as_ref()
                .cloned()
                .unwrap_or_else(|| resolved_target.clone());
            let new_relative = pathdiff::diff_paths(&abs_target, &agents_dir)
                .unwrap_or_else(|| abs_target.clone());

            match crate::link::create_symlink(&new_relative, &managed_skill) {
                Ok(()) => {
                    // Remove original from .gemini/skills/.
                    if let Err(err) = crate::link::remove_symlink(&path) {
                        warn!(
                            "migrated to {} but failed to remove {}: {err}",
                            managed_skill.display(),
                            path.display()
                        );
                    }
                    result.moved.push(GeminiMigrateEntry {
                        gemini_path: path,
                        agents_path: managed_skill,
                        target: new_relative,
                    });
                }
                Err(err) => {
                    result.move_failures.push(GeminiCleanupFailure {
                        path,
                        error: format!(
                            "cannot create symlink at {}: {err}",
                            managed_skill.display()
                        ),
                    });
                }
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
