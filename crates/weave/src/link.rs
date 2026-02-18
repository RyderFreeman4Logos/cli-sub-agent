//! Automatic skill symlinking for pattern companion skills.
//!
//! After `weave install`, each installed package may contain patterns with
//! companion skills (`patterns/<name>/skills/<name>/SKILL.md`).  These skills
//! serve as entry points for orchestrators (Claude Code, Codex, etc.) that
//! only discover skills in specific directories (`.claude/skills/`, etc.).
//!
//! This module creates relative symlinks from those directories into the
//! global package store, so orchestrators can find and invoke patterns.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{debug, warn};

use crate::check::DEFAULT_CHECK_DIRS;
use crate::package::{
    Lockfile, SourceKind, find_lockfile, global_store_root, load_lockfile, package_dir,
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Where to create skill symlinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkScope {
    /// `.claude/skills/` etc. relative to project root.
    Project,
    /// `~/.claude/skills/` etc. relative to home directory.
    User,
    /// Do not create any symlinks.
    None,
}

/// A single skill that was discovered in a package's patterns directory.
#[derive(Debug, Clone)]
pub struct DiscoveredSkill {
    /// Skill name (basename of the skill directory).
    pub name: String,
    /// Package that provides this skill.
    pub package_name: String,
    /// Absolute path to the skill directory in the global store.
    pub source_dir: PathBuf,
}

/// Outcome of a single link operation.
#[derive(Debug)]
pub enum LinkOutcome {
    /// Symlink was created.
    Created { name: String, target: PathBuf },
    /// Symlink already exists pointing to the correct target.
    Skipped { name: String },
    /// Broken symlink was replaced.
    Replaced { name: String, target: PathBuf },
}

/// Error for a single link operation.
#[derive(Debug)]
pub struct LinkError {
    /// Skill name that failed.
    pub name: String,
    /// What went wrong.
    pub reason: LinkErrorKind,
}

/// Specific link failure reasons.
#[derive(Debug)]
pub enum LinkErrorKind {
    /// Two packages expose the same skill name.
    Conflict {
        existing_package: String,
        new_package: String,
    },
    /// Target path exists and is not a symlink (regular file or directory).
    NotASymlink { path: PathBuf },
    /// Symlink exists but points to a different (non-weave-managed) target.
    ForeignSymlink { path: PathBuf, target: PathBuf },
    /// I/O or other error.
    Io(String),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.reason {
            LinkErrorKind::Conflict {
                existing_package,
                new_package,
            } => write!(
                f,
                "skill '{}' conflicts: '{}' and '{}' both expose it. \
                 Install with --no-link, then manually create renamed symlinks.",
                self.name, existing_package, new_package
            ),
            LinkErrorKind::NotASymlink { path } => write!(
                f,
                "cannot link skill '{}': {} exists and is not a symlink. \
                 Remove it manually or use --force.",
                self.name,
                path.display()
            ),
            LinkErrorKind::ForeignSymlink { path, target } => write!(
                f,
                "cannot link skill '{}': {} points to {} (not managed by weave). \
                 Remove it manually or use --force.",
                self.name,
                path.display(),
                target.display()
            ),
            LinkErrorKind::Io(msg) => write!(f, "cannot link skill '{}': {}", self.name, msg),
        }
    }
}

/// Result of a complete link operation across all packages.
#[derive(Debug, Default)]
pub struct LinkReport {
    pub outcomes: Vec<LinkOutcome>,
    pub errors: Vec<LinkError>,
}

impl LinkReport {
    pub fn created_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| {
                matches!(
                    o,
                    LinkOutcome::Created { .. } | LinkOutcome::Replaced { .. }
                )
            })
            .count()
    }

    pub fn skipped_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| matches!(o, LinkOutcome::Skipped { .. }))
            .count()
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// True if there are any conflict errors (pre-check failures).
    pub fn has_conflicts(&self) -> bool {
        self.errors
            .iter()
            .any(|e| matches!(e.reason, LinkErrorKind::Conflict { .. }))
    }
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Discover all companion skills across all installed packages.
///
/// Reads the lockfile, resolves each package's store path, and scans for
/// `patterns/*/skills/*/SKILL.md`.
pub fn discover_skills(project_root: &Path) -> Result<Vec<DiscoveredSkill>> {
    let store_root = global_store_root()?;
    let lockfile = match find_lockfile(project_root) {
        Some(path) => load_lockfile(&path)?,
        None => Lockfile {
            package: Vec::new(),
        },
    };

    let mut skills = Vec::new();

    for pkg in &lockfile.package {
        let commit_key = if pkg.source_kind == SourceKind::Local {
            "local"
        } else if pkg.commit.is_empty() {
            continue;
        } else {
            pkg.commit.as_str()
        };

        let pkg_dir = match package_dir(&store_root, &pkg.name, commit_key) {
            Ok(d) => d,
            Err(e) => {
                warn!("cannot resolve store path for '{}': {e}", pkg.name);
                continue;
            }
        };

        if !pkg_dir.is_dir() {
            debug!(
                "package '{}' not in store, skipping skill discovery",
                pkg.name
            );
            continue;
        }

        let patterns_dir = pkg_dir.join("patterns");
        if !patterns_dir.is_dir() {
            continue;
        }

        discover_skills_in_patterns(&patterns_dir, &pkg.name, &mut skills)?;
    }

    Ok(skills)
}

/// Scan `patterns/*/skills/*/SKILL.md` under a patterns directory.
fn discover_skills_in_patterns(
    patterns_dir: &Path,
    package_name: &str,
    out: &mut Vec<DiscoveredSkill>,
) -> Result<()> {
    let entries = match std::fs::read_dir(patterns_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!("cannot read {}: {e}", patterns_dir.display());
            return Ok(());
        }
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let pattern_dir = entry.path();
        if !pattern_dir.is_dir() {
            continue;
        }

        let pattern_name = match pattern_dir.file_name() {
            Some(n) => n.to_string_lossy().to_string(),
            None => continue,
        };

        let skills_dir = pattern_dir.join("skills");
        if !skills_dir.is_dir() {
            continue;
        }

        // Look for the companion skill: skills/<pattern_name>/SKILL.md
        let companion_dir = skills_dir.join(&pattern_name);
        if companion_dir.is_dir() && companion_dir.join("SKILL.md").is_file() {
            out.push(DiscoveredSkill {
                name: pattern_name,
                package_name: package_name.to_string(),
                source_dir: companion_dir,
            });
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Pre-check (conflict detection)
// ---------------------------------------------------------------------------

/// Check for naming conflicts among discovered skills.
///
/// Returns errors for any skill name that appears in multiple packages.
pub fn precheck_conflicts(skills: &[DiscoveredSkill]) -> Vec<LinkError> {
    use std::collections::HashMap;

    let mut seen: HashMap<&str, &str> = HashMap::new();
    let mut errors = Vec::new();

    for skill in skills {
        if let Some(&existing_pkg) = seen.get(skill.name.as_str()) {
            if existing_pkg != skill.package_name {
                errors.push(LinkError {
                    name: skill.name.clone(),
                    reason: LinkErrorKind::Conflict {
                        existing_package: existing_pkg.to_string(),
                        new_package: skill.package_name.clone(),
                    },
                });
            }
        } else {
            seen.insert(&skill.name, &skill.package_name);
        }
    }

    errors
}

// ---------------------------------------------------------------------------
// Linking
// ---------------------------------------------------------------------------

/// Create symlinks for all discovered skills in the appropriate directories.
///
/// `scope` determines where symlinks are created (project or user level).
/// `force` overwrites existing non-weave symlinks.
pub fn link_skills(project_root: &Path, scope: LinkScope, force: bool) -> Result<LinkReport> {
    if scope == LinkScope::None {
        return Ok(LinkReport::default());
    }

    let skills = discover_skills(project_root)?;

    // Pre-check for conflicts.
    let conflicts = precheck_conflicts(&skills);
    if !conflicts.is_empty() {
        return Ok(LinkReport {
            outcomes: Vec::new(),
            errors: conflicts,
        });
    }

    let store_root = global_store_root()?;
    let base_dir = match scope {
        LinkScope::Project => project_root.to_path_buf(),
        LinkScope::User => {
            let dirs = directories::BaseDirs::new().context("cannot determine home directory")?;
            dirs.home_dir().to_path_buf()
        }
        LinkScope::None => unreachable!(),
    };

    let mut report = LinkReport::default();

    for target_dir_name in DEFAULT_CHECK_DIRS {
        let target_dir = base_dir.join(target_dir_name);

        // Decide whether to create this target directory.
        // - The primary directory (.claude/skills/) is always created — it is
        //   the standard discovery path and must exist for first-time setups.
        // - Other tool directories are only created if their parent already
        //   exists (e.g., create .codex/skills/ only if .codex/ is present).
        let is_primary = *target_dir_name == DEFAULT_CHECK_DIRS[0];
        let should_create =
            target_dir.is_dir() || is_primary || target_dir.parent().is_some_and(|p| p.is_dir());

        if !should_create {
            continue;
        }

        if !target_dir.exists() {
            std::fs::create_dir_all(&target_dir)
                .with_context(|| format!("cannot create {}", target_dir.display()))?;
        }

        for skill in &skills {
            let link_path = target_dir.join(&skill.name);
            let outcome = create_skill_link(
                &link_path,
                &skill.source_dir,
                &target_dir,
                &store_root,
                skill,
                force,
            );

            match outcome {
                Ok(o) => report.outcomes.push(o),
                Err(e) => report.errors.push(e),
            }
        }
    }

    Ok(report)
}

/// Create or validate a single skill symlink.
fn create_skill_link(
    link_path: &Path,
    source_dir: &Path,
    link_parent: &Path,
    store_root: &Path,
    skill: &DiscoveredSkill,
    force: bool,
) -> Result<LinkOutcome, LinkError> {
    // Compute relative path from link location to source.
    let relative_target = pathdiff::diff_paths(source_dir, link_parent).unwrap_or_else(|| {
        // Fallback to absolute if relative computation fails.
        source_dir.to_path_buf()
    });

    // Check if something already exists at the link path.
    match std::fs::symlink_metadata(link_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            // Existing symlink — check where it points.
            let existing_target = std::fs::read_link(link_path).map_err(|e| LinkError {
                name: skill.name.clone(),
                reason: LinkErrorKind::Io(format!("cannot read symlink: {e}")),
            })?;

            let resolved_existing = if existing_target.is_absolute() {
                existing_target.clone()
            } else {
                link_parent.join(&existing_target)
            };

            let resolved_new = if relative_target.is_absolute() {
                relative_target.clone()
            } else {
                link_parent.join(&relative_target)
            };

            // Same effective target → skip.
            if paths_equivalent(&resolved_existing, &resolved_new) {
                return Ok(LinkOutcome::Skipped {
                    name: skill.name.clone(),
                });
            }

            // Existing symlink points into weave store → safe to replace.
            let is_weave_managed = is_weave_managed_path(&resolved_existing, store_root);

            // Check if target is broken (broken symlinks are always safe to replace).
            let is_broken = !resolved_existing.try_exists().unwrap_or(false);

            if is_broken || is_weave_managed || force {
                remove_symlink(link_path).map_err(|e| LinkError {
                    name: skill.name.clone(),
                    reason: LinkErrorKind::Io(format!("cannot remove old symlink: {e}")),
                })?;
                create_symlink(&relative_target, link_path).map_err(|e| LinkError {
                    name: skill.name.clone(),
                    reason: LinkErrorKind::Io(format!("cannot create symlink: {e}")),
                })?;
                return Ok(LinkOutcome::Replaced {
                    name: skill.name.clone(),
                    target: relative_target,
                });
            }

            // Foreign symlink — cannot replace without --force.
            Err(LinkError {
                name: skill.name.clone(),
                reason: LinkErrorKind::ForeignSymlink {
                    path: link_path.to_path_buf(),
                    target: existing_target,
                },
            })
        }
        Ok(_meta) => {
            // Exists but is not a symlink (regular file or directory).
            if force {
                // Force mode: remove and replace.
                if link_path.is_dir() {
                    std::fs::remove_dir_all(link_path).map_err(|e| LinkError {
                        name: skill.name.clone(),
                        reason: LinkErrorKind::Io(format!("cannot remove directory: {e}")),
                    })?;
                } else {
                    std::fs::remove_file(link_path).map_err(|e| LinkError {
                        name: skill.name.clone(),
                        reason: LinkErrorKind::Io(format!("cannot remove file: {e}")),
                    })?;
                }
                create_symlink(&relative_target, link_path).map_err(|e| LinkError {
                    name: skill.name.clone(),
                    reason: LinkErrorKind::Io(format!("cannot create symlink: {e}")),
                })?;
                Ok(LinkOutcome::Replaced {
                    name: skill.name.clone(),
                    target: relative_target,
                })
            } else {
                Err(LinkError {
                    name: skill.name.clone(),
                    reason: LinkErrorKind::NotASymlink {
                        path: link_path.to_path_buf(),
                    },
                })
            }
        }
        Err(_) => {
            // Nothing exists — create fresh symlink.
            create_symlink(&relative_target, link_path).map_err(|e| LinkError {
                name: skill.name.clone(),
                reason: LinkErrorKind::Io(format!("cannot create symlink: {e}")),
            })?;
            Ok(LinkOutcome::Created {
                name: skill.name.clone(),
                target: relative_target,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Sync (reconcile)
// ---------------------------------------------------------------------------

/// Detect stale symlinks that point into the weave store but whose skill
/// is no longer tracked by any installed package. A link is NOT stale if:
/// - Its basename matches a known skill name, OR
/// - Its target resolves to a known skill source directory (handles renames).
///
/// Returns paths without modifying the filesystem.
pub fn detect_stale_links(project_root: &Path, scope: LinkScope) -> Result<Vec<PathBuf>> {
    if scope == LinkScope::None {
        return Ok(Vec::new());
    }

    let store_root = global_store_root()?;
    let skills = discover_skills(project_root)?;
    let skill_names: std::collections::HashSet<&str> =
        skills.iter().map(|s| s.name.as_str()).collect();
    // Collect canonicalized source dirs so renamed symlinks are preserved.
    let skill_source_dirs: std::collections::HashSet<PathBuf> = skills
        .iter()
        .filter_map(|s| s.source_dir.canonicalize().ok())
        .collect();

    let base_dir = scope_base_dir(project_root, scope)?;

    let mut stale = Vec::new();

    for dir_name in DEFAULT_CHECK_DIRS {
        let dir = base_dir.join(dir_name);
        if !dir.is_dir() {
            continue;
        }

        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if is_stale_link(&path, &store_root, &skill_names, &skill_source_dirs) {
                stale.push(path);
            }
        }
    }

    Ok(stale)
}

/// Remove stale symlinks that point into the weave store but whose package
/// is no longer in the lockfile.
pub fn remove_stale_links(project_root: &Path, scope: LinkScope) -> Result<Vec<PathBuf>> {
    let stale = detect_stale_links(project_root, scope)?;

    let mut removed = Vec::new();
    for path in stale {
        match remove_symlink(&path) {
            Ok(()) => removed.push(path),
            Err(e) => {
                eprintln!(
                    "warning: failed to remove stale symlink {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }

    Ok(removed)
}

/// Resolve scope to a base directory.
fn scope_base_dir(project_root: &Path, scope: LinkScope) -> Result<PathBuf> {
    match scope {
        LinkScope::Project => Ok(project_root.to_path_buf()),
        LinkScope::User => {
            let dirs = directories::BaseDirs::new().context("cannot determine home directory")?;
            Ok(dirs.home_dir().to_path_buf())
        }
        LinkScope::None => unreachable!(),
    }
}

/// Check if a symlink is stale (points into weave store but is no longer
/// associated with any installed skill — by name or by target path).
///
/// A link is NOT stale if:
/// - Its basename matches a known skill name, OR
/// - Its resolved target matches a known skill source directory (renamed link).
fn is_stale_link(
    path: &Path,
    store_root: &Path,
    skill_names: &std::collections::HashSet<&str>,
    skill_source_dirs: &std::collections::HashSet<PathBuf>,
) -> bool {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };

    if !meta.file_type().is_symlink() {
        return false;
    }

    let target = match std::fs::read_link(path) {
        Ok(t) => t,
        Err(_) => return false,
    };

    let resolved = if target.is_absolute() {
        target
    } else {
        let parent = path.parent().unwrap_or(Path::new("."));
        parent.join(&target)
    };

    if !is_weave_managed_path(&resolved, store_root) {
        return false;
    }

    // Check 1: basename matches a known skill name.
    let link_name = match path.file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => return false,
    };
    if skill_names.contains(link_name.as_str()) {
        return false;
    }

    // Check 2: target resolves to a known skill source directory (renamed link).
    if let Ok(canonical) = resolved.canonicalize() {
        if skill_source_dirs.contains(&canonical) {
            return false;
        }
    }

    true
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if two paths refer to the same location (after resolution).
fn paths_equivalent(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

/// Check if a path is inside the weave global store.
fn is_weave_managed_path(path: &Path, store_root: &Path) -> bool {
    match (path.canonicalize(), store_root.canonicalize()) {
        (Ok(cp), Ok(cs)) => cp.starts_with(&cs),
        _ => {
            // Fallback for broken symlinks: normalize `..` segments via
            // component-based cleanup so relative resolution still matches.
            let np = normalize_path(path);
            let ns = normalize_path(store_root);
            np.starts_with(&ns)
        }
    }
}

/// Normalize a path by resolving `.` and `..` components lexically (no I/O).
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// Create a symlink (platform-specific).
#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    // Companion skills are directories, so use symlink_dir.
    std::os::windows::fs::symlink_dir(target, link)
}

/// Remove a symlink (cross-platform).
fn remove_symlink(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path))
}

#[cfg(test)]
#[path = "link_tests.rs"]
mod tests;
