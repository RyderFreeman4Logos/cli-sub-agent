//! Audit installed skills for consistency issues.
//!
//! Split from `package.rs` to stay under the monolith-file limit.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{
    Lockfile, SourceKind, detect_skill_md_case_mismatch, load_project_lockfile, package_dir,
};

/// Audit result for a single package.
#[derive(Debug)]
pub struct AuditResult {
    pub name: String,
    pub issues: Vec<AuditIssue>,
}

/// A single audit issue.
#[derive(Debug)]
pub enum AuditIssue {
    /// Dependency in lockfile but missing from the global package store.
    MissingFromDeps,
    /// Dependency present in store but not in lockfile.
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
            Self::MissingFromDeps => write!(f, "locked but missing from global package store"),
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
///
/// Checks packages in the lockfile against the global store at `store_root`.
pub fn audit(project_root: &Path, store_root: &Path) -> Result<Vec<AuditResult>> {
    let lockfile = load_project_lockfile(project_root).unwrap_or(Lockfile {
        package: Vec::new(),
    });

    let mut results = Vec::new();

    // Check each locked package against the global store.
    for pkg in &lockfile.package {
        let mut issues = Vec::new();

        // Determine the checkout directory in the global store.
        let commit_key = if pkg.source_kind == SourceKind::Local {
            "local"
        } else if pkg.commit.is_empty() {
            ""
        } else {
            pkg.commit.as_str()
        };

        if commit_key.is_empty() {
            // No commit or local key — cannot locate in store.
            if pkg.repo.is_empty() && pkg.source_kind != SourceKind::Local {
                issues.push(AuditIssue::UnknownRepo);
            }
            issues.push(AuditIssue::MissingFromDeps);
        } else {
            let dep_path = package_dir(store_root, &pkg.name, commit_key);

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
        }

        if !issues.is_empty() {
            results.push(AuditResult {
                name: pkg.name.clone(),
                issues,
            });
        }
    }

    Ok(results)
}
