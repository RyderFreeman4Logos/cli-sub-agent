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

/// Opaque journal revision identifier.
///
/// This is intentionally distinct from [`VcsIdentity`]: snapshot journals only
/// need a stable handle for their sidecar history, while canonical repository
/// operations may need richer git/jj identity metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RevisionId(pub String);

impl RevisionId {
    /// Returns the underlying revision identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for RevisionId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for RevisionId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl std::fmt::Display for RevisionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Errors returned by snapshot journal implementations.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum JournalError {
    /// The configured journal backend is unavailable in the current environment.
    #[error("snapshot journal unavailable: {0}")]
    Unavailable(String),

    /// An operating-system or filesystem failure occurred.
    #[error("snapshot journal I/O failure: {0}")]
    Io(String),

    /// The journal command failed.
    #[error("{command} failed: {message}")]
    CommandFailed { command: String, message: String },

    /// The journal state file contents were invalid.
    #[error("snapshot journal state invalid: {0}")]
    InvalidState(String),

    /// The requested snapshot message was invalid after sanitization.
    #[error("snapshot journal message invalid: {0}")]
    InvalidMessage(String),
}

impl From<std::io::Error> for JournalError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
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

    /// Detect the repository's default branch, if it can be determined.
    fn default_branch(&self, project_root: &Path) -> Result<Option<String>, String>;

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

/// Sidecar snapshot journaling contract.
///
/// [`VcsBackend`] handles canonical repository operations; `SnapshotJournal` is
/// a parallel sidecar contract for journaling. They are NOT supertypes.
pub trait SnapshotJournal: Send + Sync {
    /// Capture the current working-tree state into the sidecar journal.
    fn snapshot(&self, message: &str) -> Result<RevisionId, JournalError>;

    /// Return the first journal revision recorded for the current session, if any.
    fn session_start_revision(&self) -> Result<Option<RevisionId>, JournalError>;
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

    #[derive(Default)]
    struct DummyJournal;

    impl SnapshotJournal for DummyJournal {
        fn snapshot(&self, message: &str) -> Result<RevisionId, JournalError> {
            Ok(RevisionId::from(format!("snap:{message}")))
        }

        fn session_start_revision(&self) -> Result<Option<RevisionId>, JournalError> {
            Ok(Some(RevisionId::from("start-rev")))
        }
    }

    #[test]
    fn snapshot_journal_trait_surface_roundtrips_revision_ids() {
        let journal: &dyn SnapshotJournal = &DummyJournal;
        let revision = journal.snapshot("hello").expect("snapshot should succeed");
        let start = journal
            .session_start_revision()
            .expect("state read should succeed")
            .expect("start revision should exist");

        assert_eq!(revision.as_str(), "snap:hello");
        assert_eq!(start.as_str(), "start-rev");
    }

    #[test]
    fn revision_id_serde_roundtrip() {
        let revision = RevisionId::from("kqosnzyt");
        let json = serde_json::to_string(&revision).expect("serialize revision id");
        let decoded: RevisionId = serde_json::from_str(&json).expect("deserialize revision id");
        assert_eq!(decoded, revision);
    }
}
