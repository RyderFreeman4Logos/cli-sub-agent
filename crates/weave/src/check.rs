//! Symlink health checking for skill directories.
//!
//! Scans tool-specific skill directories (`.claude/skills/`, `.codex/skills/`,
//! etc.) for broken symlinks and optionally removes them.

use std::collections::HashSet;
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
        // Track visited inodes to detect symlink cycles.
        let mut visited = HashSet::new();

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

            // Cycle detection via inode.
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let inode = meta.ino();
                if !visited.insert(inode) {
                    continue; // Already seen this inode.
                }
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

            // Check if target exists (without following further symlinks).
            if !resolved.exists() {
                issues.push(AuditIssue::BrokenSymlink {
                    path: path.clone(),
                    target: target.clone(),
                });

                if fix {
                    // Only remove the symlink itself, never follow it.
                    if let Ok(m) = std::fs::symlink_metadata(&path) {
                        if m.file_type().is_symlink() {
                            match std::fs::remove_file(&path) {
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
