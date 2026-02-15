//! Symlink health checking for skill directories.
//!
//! Scans tool-specific skill directories (`.claude/skills/`, `.codex/skills/`,
//! etc.) for broken symlinks and optionally removes them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::warn;

use crate::package::AuditIssue;

/// Default directories to scan for broken symlinks.
pub const DEFAULT_CHECK_DIRS: &[&str] = &[
    ".claude/skills",
    ".codex/skills",
    ".agents/skills",
    ".gemini/skills",
];

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
                    if let Ok(m) = std::fs::symlink_metadata(&path) {
                        if m.file_type().is_symlink() {
                            // Try remove_file first (works for file symlinks
                            // on all platforms).  Fall back to remove_dir for
                            // Windows directory symlinks/junctions.
                            match std::fs::remove_file(&path)
                                .or_else(|_| std::fs::remove_dir(&path))
                            {
                                Ok(()) => fixed += 1,
                                Err(_) => fix_failures += 1,
                            }
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

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
