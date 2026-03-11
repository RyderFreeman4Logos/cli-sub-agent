//! Version control system abstraction layer.
//!
//! Provides a trait-based interface for VCS operations used by CSA,
//! allowing git and jj (Jujutsu) to be used interchangeably.

use std::path::Path;

/// Detected VCS backend kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsKind {
    Git,
    Jj,
}

impl std::fmt::Display for VcsKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VcsKind::Git => write!(f, "git"),
            VcsKind::Jj => write!(f, "jj"),
        }
    }
}

/// Common VCS operations required by CSA session and todo management.
///
/// Each method returns `Result<T, String>` since csa-core does not depend on anyhow.
/// Implementors should return human-readable error messages.
pub trait VcsBackend: Send + Sync {
    /// The backend kind (git or jj).
    fn kind(&self) -> VcsKind;

    /// Get the current branch name, if any.
    fn current_branch(&self, project_root: &Path) -> Result<Option<String>, String>;

    /// Get the current HEAD commit hash (full SHA for git, change-id for jj).
    fn head_id(&self, project_root: &Path) -> Result<Option<String>, String>;

    /// Get a short identifier suitable for display (e.g., short SHA or change-id prefix).
    fn head_short_id(&self, project_root: &Path) -> Result<Option<String>, String>;

    /// Initialize a VCS repository if one doesn't exist.
    fn init(&self, path: &Path) -> Result<(), String>;

    /// Stage a file for the next commit.
    fn add(&self, project_root: &Path, path: &Path) -> Result<(), String>;

    /// Create a commit with the given message.
    fn commit(&self, project_root: &Path, message: &str) -> Result<(), String>;

    /// Get the diff of uncommitted changes.
    fn diff_uncommitted(&self, project_root: &Path) -> Result<String, String>;
}

/// Detect which VCS backend to use for the given project root.
///
/// Priority: jj (if `.jj/` exists) > git (if `.git/` exists or `git rev-parse` succeeds).
pub fn detect_vcs_kind(project_root: &Path) -> Option<VcsKind> {
    if project_root.join(".jj").is_dir() {
        Some(VcsKind::Jj)
    } else if project_root.join(".git").exists() {
        Some(VcsKind::Git)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_temp_dir(suffix: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("csa-vcs-test-{}-{}", suffix, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detect_vcs_kind_prefers_jj_over_git() {
        let temp = make_temp_dir("jj-git");
        fs::create_dir(temp.join(".git")).unwrap();
        fs::create_dir(temp.join(".jj")).unwrap();

        assert_eq!(detect_vcs_kind(&temp), Some(VcsKind::Jj));
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn detect_vcs_kind_falls_back_to_git() {
        let temp = make_temp_dir("git-only");
        fs::create_dir(temp.join(".git")).unwrap();

        assert_eq!(detect_vcs_kind(&temp), Some(VcsKind::Git));
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn detect_vcs_kind_returns_none_for_no_vcs() {
        let temp = make_temp_dir("no-vcs");

        assert_eq!(detect_vcs_kind(&temp), None);
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn vcs_kind_display() {
        assert_eq!(VcsKind::Git.to_string(), "git");
        assert_eq!(VcsKind::Jj.to_string(), "jj");
    }
}
