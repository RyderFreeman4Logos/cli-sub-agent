//! Version control system abstraction layer.
//!
//! Provides a trait-based interface for VCS operations used by CSA,
//! allowing git and jj (Jujutsu) to be used interchangeably.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Detected VCS backend kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VcsKind {
    #[default]
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

/// Unified VCS identity capturing both Git and jj metadata.
///
/// # Semantic invariants
///
/// - `commit_id` is the immutable content hash (Git SHA or jj commit id).
///   Review freshness MUST be based on this field, never on `change_id`.
/// - `change_id` is the logical identity (jj only). It is stable across rebases
///   but MUST NOT be used for content freshness checks — doing so would allow
///   stale reviews to pass after content-changing operations like `jj amend`.
/// - `op_id` is the jj operation log ID. It detects repository-wide state changes
///   (e.g., `jj undo`, `jj restore`) even when commit_id happens to match.
/// - At least one of `commit_id` or `change_id` MUST be `Some`.
/// - Mixing `commit_id` and `change_id` for equality comparison is a bug.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VcsIdentity {
    #[serde(default)]
    pub vcs_kind: VcsKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_id: Option<String>,
}

impl VcsIdentity {
    /// Returns `true` if all optional fields are `None` (default/empty identity).
    pub fn is_default(&self) -> bool {
        self.commit_id.is_none()
            && self.change_id.is_none()
            && self.short_id.is_none()
            && self.ref_name.is_none()
            && self.op_id.is_none()
    }

    /// Asserts that this identity has at least one meaningful identifier.
    /// Panics in debug builds if both `commit_id` and `change_id` are `None`.
    pub fn assert_valid(&self) {
        debug_assert!(
            self.commit_id.is_some() || self.change_id.is_some(),
            "VcsIdentity must have at least one of commit_id or change_id"
        );
    }
}

impl std::fmt::Display for VcsIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.vcs_kind {
            VcsKind::Git => {
                let id = self
                    .short_id
                    .as_deref()
                    .or(self.commit_id.as_deref())
                    .unwrap_or("unknown");
                write!(f, "[git:{id}]")
            }
            VcsKind::Jj => {
                let id = self
                    .short_id
                    .as_deref()
                    .or(self.change_id.as_deref())
                    .unwrap_or("unknown");
                write!(f, "[jj:{id}]")
            }
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

    /// Get a unified VCS identity snapshot for the current working copy.
    ///
    /// Default implementation assembles identity from individual methods.
    /// Backends should override this for efficiency (single subprocess call).
    fn identity(&self, project_root: &Path) -> Result<VcsIdentity, String> {
        Ok(VcsIdentity {
            vcs_kind: self.kind(),
            commit_id: self.head_id(project_root).ok().flatten(),
            change_id: None,
            short_id: self.head_short_id(project_root).ok().flatten(),
            ref_name: self.current_branch(project_root).ok().flatten(),
            op_id: None,
        })
    }
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

    #[test]
    fn test_vcs_identity_default() {
        let id = VcsIdentity::default();
        assert_eq!(id.vcs_kind, VcsKind::Git);
        assert!(id.commit_id.is_none());
        assert!(id.change_id.is_none());
        assert!(id.short_id.is_none());
        assert!(id.ref_name.is_none());
        assert!(id.op_id.is_none());
    }

    #[test]
    fn test_vcs_identity_is_default() {
        assert!(VcsIdentity::default().is_default());

        let with_commit = VcsIdentity {
            commit_id: Some("abc123".into()),
            ..Default::default()
        };
        assert!(!with_commit.is_default());
    }

    #[test]
    fn test_vcs_identity_display_git() {
        let id = VcsIdentity {
            vcs_kind: VcsKind::Git,
            commit_id: Some("abc123def456".into()),
            short_id: Some("abc123d".into()),
            ref_name: Some("main".into()),
            ..Default::default()
        };
        assert_eq!(id.to_string(), "[git:abc123d]");
    }

    #[test]
    fn test_vcs_identity_display_jj() {
        let id = VcsIdentity {
            vcs_kind: VcsKind::Jj,
            change_id: Some("kxmlopqr".into()),
            short_id: Some("kxmlo".into()),
            ..Default::default()
        };
        assert_eq!(id.to_string(), "[jj:kxmlo]");
    }

    #[test]
    fn test_vcs_identity_serde_roundtrip() {
        let id = VcsIdentity {
            vcs_kind: VcsKind::Jj,
            commit_id: Some("deadbeef".into()),
            change_id: Some("kxmlopqr".into()),
            short_id: Some("kxmlo".into()),
            ref_name: Some("my-bookmark".into()),
            op_id: Some("op123".into()),
        };
        let json = serde_json::to_string(&id).unwrap();
        let roundtripped: VcsIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(id, roundtripped);
    }
}
