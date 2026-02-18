//! Audit installed skills for consistency issues.
//!
//! Split from `package.rs` to stay under the monolith-file limit.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{LockedPackage, Lockfile, SourceKind, detect_skill_md_case_mismatch, load_project_lockfile};

/// Audit result for a single package.
#[derive(Debug)]
pub struct AuditResult {
    pub name: String,
    pub issues: Vec<AuditIssue>,
}

/// A single audit issue.
#[derive(Debug)]
pub enum AuditIssue {
    /// Dependency in lockfile but missing from `.weave/deps/`.
    MissingFromDeps,
    /// Dependency in `.weave/deps/` but not in lockfile.
    MissingFromLockfile,
    /// Empty repo URL in lockfile — not installed via weave.
    UnknownRepo,
    /// SKILL.md not found in dependency directory.
    MissingSkillMd,
    /// A case-variant of `SKILL.md` exists (e.g. `skill.md`, `Skill.md`)
    /// but the canonical `SKILL.md` is missing.
    CaseMismatchSkillMd {
        /// The actual filename found on disk.
        found: String,
    },
    /// Symlink target does not exist.
    BrokenSymlink {
        /// Path of the broken symlink.
        path: PathBuf,
        /// Target the symlink points to.
        target: PathBuf,
    },
}

impl std::fmt::Display for AuditIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingFromDeps => write!(f, "locked but missing from .weave/deps/"),
            Self::MissingFromLockfile => write!(f, "present in deps but not in lockfile"),
            Self::UnknownRepo => write!(f, "lockfile entry has no repo URL"),
            Self::MissingSkillMd => write!(f, "no SKILL.md found"),
            Self::CaseMismatchSkillMd { found } => {
                write!(
                    f,
                    "expected 'SKILL.md' but found '{found}' (wrong case). Rename to 'SKILL.md' to fix."
                )
            }
            Self::BrokenSymlink { path, target } => {
                write!(
                    f,
                    "broken symlink: {} -> {}",
                    path.display(),
                    target.display()
                )
            }
        }
    }
}

/// Audit installed skills for consistency issues.
pub fn audit(project_root: &Path) -> Result<Vec<AuditResult>> {
    let deps_dir = project_root.join(".weave").join("deps");

    let lockfile = load_project_lockfile(project_root).unwrap_or(Lockfile {
        package: Vec::new(),
    });

    let locked_names: BTreeMap<String, &LockedPackage> = lockfile
        .package
        .iter()
        .map(|p| (p.name.clone(), p))
        .collect();

    let mut results = Vec::new();

    // Check each locked package.
    for pkg in &lockfile.package {
        let mut issues = Vec::new();
        let dep_path = deps_dir.join(&pkg.name);

        if !dep_path.is_dir() {
            issues.push(AuditIssue::MissingFromDeps);
        } else if !dep_path.join("SKILL.md").is_file() {
            // Distinguish case-mismatch from truly missing.
            if let Some(found) = detect_skill_md_case_mismatch(&dep_path) {
                issues.push(AuditIssue::CaseMismatchSkillMd { found });
            } else {
                issues.push(AuditIssue::MissingSkillMd);
            }
        }

        if pkg.repo.is_empty() && pkg.source_kind != SourceKind::Local {
            issues.push(AuditIssue::UnknownRepo);
        }

        if !issues.is_empty() {
            results.push(AuditResult {
                name: pkg.name.clone(),
                issues,
            });
        }
    }

    // Check for deps not in lockfile.
    if deps_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&deps_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                if entry.path().is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !locked_names.contains_key(&name) {
                        results.push(AuditResult {
                            name,
                            issues: vec![AuditIssue::MissingFromLockfile],
                        });
                    }
                }
            }
        }
    }

    Ok(results)
}
