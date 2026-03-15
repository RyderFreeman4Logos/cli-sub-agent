//! Concrete VCS backend implementations for git and jj.
//!
//! # Security Model
//!
//! All commands use `std::process::Command` with `.args()` for argument passing,
//! which prevents shell injection by design. Arguments are passed as separate
//! OS strings, never through shell expansion. User-provided inputs (commit
//! messages, paths) are passed via `.args()` or `.arg()`, not string interpolation.
//! Template strings for jj `-T` flags are hardcoded constants.

use csa_core::vcs::{VcsBackend, VcsIdentity, VcsKind};
use std::path::Path;
use std::process::Command;

/// Git VCS backend.
pub struct GitBackend;

impl VcsBackend for GitBackend {
    fn kind(&self) -> VcsKind {
        VcsKind::Git
    }

    fn current_branch(&self, project_root: &Path) -> Result<Option<String>, String> {
        let output = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run git rev-parse: {e}"))?;

        if !output.status.success() {
            return Ok(None);
        }

        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if branch.is_empty() || branch == "HEAD" {
            Ok(None)
        } else {
            Ok(Some(branch))
        }
    }

    fn head_id(&self, project_root: &Path) -> Result<Option<String>, String> {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run git rev-parse HEAD: {e}"))?;

        if !output.status.success() {
            return Ok(None);
        }

        let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if head.is_empty() {
            Ok(None)
        } else {
            Ok(Some(head))
        }
    }

    fn head_short_id(&self, project_root: &Path) -> Result<Option<String>, String> {
        let output = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run git rev-parse --short HEAD: {e}"))?;

        if !output.status.success() {
            return Ok(None);
        }

        let short = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if short.is_empty() {
            Ok(None)
        } else {
            Ok(Some(short))
        }
    }

    fn identity(&self, project_root: &Path) -> Result<VcsIdentity, String> {
        // Single git call to get HEAD SHA, short SHA, and branch in one invocation
        // is not straightforward with git, so we make two calls (rev-parse for SHA+short,
        // rev-parse --abbrev-ref for branch). This is still better than 3 separate calls.
        let commit_id = self.head_id(project_root).ok().flatten();
        let short_id = self.head_short_id(project_root).ok().flatten();
        let ref_name = self.current_branch(project_root).ok().flatten();

        Ok(VcsIdentity {
            vcs_kind: VcsKind::Git,
            commit_id,
            change_id: None, // Git has no logical change identity
            short_id,
            ref_name,
            op_id: None, // Git has no operation log
        })
    }

    fn init(&self, path: &Path) -> Result<(), String> {
        let output = Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .map_err(|e| format!("Failed to run git init: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "git init failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn add(&self, project_root: &Path, path: &Path) -> Result<(), String> {
        let output = Command::new("git")
            .args(["add", "--"])
            .arg(path)
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run git add: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "git add failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn commit(&self, project_root: &Path, message: &str) -> Result<(), String> {
        validate_commit_message(message)?;
        let output = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run git commit: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "git commit failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn diff_uncommitted(&self, project_root: &Path) -> Result<String, String> {
        let output = Command::new("git")
            .args(["diff"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run git diff: {e}"))?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Jujutsu (jj) VCS backend.
pub struct JjBackend;

impl VcsBackend for JjBackend {
    fn kind(&self) -> VcsKind {
        VcsKind::Jj
    }

    fn current_branch(&self, project_root: &Path) -> Result<Option<String>, String> {
        // jj uses bookmarks instead of branches
        let output = Command::new("jj")
            .args(["bookmark", "list", "--no-pager", "-r", "@"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run jj bookmark list: {e}"))?;

        if !output.status.success() {
            return Ok(None);
        }

        let bookmarks = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // First line is the primary bookmark name (before any ':' or whitespace)
        bookmarks
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().next())
            .filter(|s| !s.is_empty())
            .map_or(Ok(None), |b| Ok(Some(b.to_string())))
    }

    fn head_id(&self, project_root: &Path) -> Result<Option<String>, String> {
        let output = Command::new("jj")
            .args(["log", "--no-graph", "-r", "@", "-T", "change_id"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run jj log: {e}"))?;

        if !output.status.success() {
            return Ok(None);
        }

        let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if id.is_empty() {
            Ok(None)
        } else {
            Ok(Some(id))
        }
    }

    fn head_short_id(&self, project_root: &Path) -> Result<Option<String>, String> {
        let output = Command::new("jj")
            .args([
                "log",
                "--no-graph",
                "-r",
                "@",
                "-T",
                "change_id.shortest(8)",
            ])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run jj log: {e}"))?;

        if !output.status.success() {
            return Ok(None);
        }

        let short = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if short.is_empty() {
            Ok(None)
        } else {
            Ok(Some(short))
        }
    }

    fn identity(&self, project_root: &Path) -> Result<VcsIdentity, String> {
        // Get change_id, commit_id, and short change_id in one jj call
        let id_output = Command::new("jj")
            .args([
                "log",
                "--no-graph",
                "-r",
                "@",
                "-T",
                r#"change_id ++ "\n" ++ commit_id ++ "\n" ++ change_id.shortest(8)"#,
            ])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run jj log: {e}"))?;

        let (change_id, commit_id, short_id) = if id_output.status.success() {
            let text = String::from_utf8_lossy(&id_output.stdout);
            let mut lines = text.trim().lines();
            let cid = lines
                .next()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let cmid = lines
                .next()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let sid = lines
                .next()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            (cid, cmid, sid)
        } else {
            (None, None, None)
        };

        // Get bookmark (branch equivalent)
        let ref_name = self.current_branch(project_root).ok().flatten();

        // Get current operation ID
        let op_output = Command::new("jj")
            .args([
                "op",
                "log",
                "--no-graph",
                "-l",
                "1",
                "-T",
                "self.id().short(12)",
            ])
            .current_dir(project_root)
            .output()
            .ok();
        let op_id = op_output
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(VcsIdentity {
            vcs_kind: VcsKind::Jj,
            commit_id,
            change_id,
            short_id,
            ref_name,
            op_id,
        })
    }

    fn init(&self, path: &Path) -> Result<(), String> {
        let output = Command::new("jj")
            .args(["git", "init"])
            .current_dir(path)
            .output()
            .map_err(|e| format!("Failed to run jj git init: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "jj git init failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn add(&self, _project_root: &Path, _path: &Path) -> Result<(), String> {
        // jj auto-tracks all files; no explicit staging needed
        Ok(())
    }

    fn commit(&self, project_root: &Path, message: &str) -> Result<(), String> {
        validate_commit_message(message)?;
        let output = Command::new("jj")
            .args(["commit", "-m", message])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run jj commit: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "jj commit failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn diff_uncommitted(&self, project_root: &Path) -> Result<String, String> {
        let output = Command::new("jj")
            .args(["diff", "--no-pager"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run jj diff: {e}"))?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Maximum commit message length (bytes). Prevents accidental or malicious oversized messages.
const MAX_COMMIT_MESSAGE_LEN: usize = 65536;

/// Validate a commit message for security and sanity.
fn validate_commit_message(message: &str) -> Result<(), String> {
    if message.contains('\0') {
        return Err("Commit message contains null byte: rejected for security".to_string());
    }
    if message.len() > MAX_COMMIT_MESSAGE_LEN {
        return Err(format!(
            "Commit message too long ({} bytes, max {})",
            message.len(),
            MAX_COMMIT_MESSAGE_LEN
        ));
    }
    Ok(())
}

/// Create the appropriate VcsBackend for the given project root.
///
/// Selection priority: explicit `backend` config > colocated_default > auto-detect.
/// For colocated repos (both `.jj/` and `.git/` present), `colocated_default`
/// overrides auto-detect's jj preference (defaults to Git when unset).
pub fn create_vcs_backend(project_root: &Path) -> Box<dyn VcsBackend> {
    create_vcs_backend_with_config(project_root, None, None)
}

/// Create a VcsBackend with explicit configuration overrides.
pub fn create_vcs_backend_with_config(
    project_root: &Path,
    backend_override: Option<VcsKind>,
    colocated_default: Option<VcsKind>,
) -> Box<dyn VcsBackend> {
    // Explicit override takes top priority
    if let Some(kind) = backend_override {
        return match kind {
            VcsKind::Jj => Box::new(JjBackend),
            VcsKind::Git => Box::new(GitBackend),
        };
    }

    let has_jj = project_root.join(".jj").is_dir();
    let has_git = project_root.join(".git").exists();

    match (has_jj, has_git) {
        // Colocated: both present — use colocated_default (defaults to Git)
        (true, true) => match colocated_default.unwrap_or(VcsKind::Git) {
            VcsKind::Jj => Box::new(JjBackend),
            VcsKind::Git => Box::new(GitBackend),
        },
        // jj only
        (true, false) => Box::new(JjBackend),
        // git only or neither (default to git)
        _ => Box::new(GitBackend),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_vcs_backend_defaults_to_git() {
        let temp =
            std::env::temp_dir().join(format!("csa-vcs-backend-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let backend = create_vcs_backend(&temp);
        assert_eq!(backend.kind(), VcsKind::Git);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn validate_commit_message_rejects_null_byte() {
        let result = validate_commit_message("hello\0world");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("null byte"));
    }

    #[test]
    fn validate_commit_message_rejects_oversized() {
        let msg = "x".repeat(MAX_COMMIT_MESSAGE_LEN + 1);
        let result = validate_commit_message(&msg);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too long"));
    }

    #[test]
    fn validate_commit_message_accepts_normal() {
        assert!(validate_commit_message("feat: add VCS abstraction layer").is_ok());
    }

    #[test]
    fn create_vcs_backend_detects_jj() {
        let temp =
            std::env::temp_dir().join(format!("csa-vcs-backend-test-jj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(temp.join(".jj")).unwrap();

        let backend = create_vcs_backend(&temp);
        assert_eq!(backend.kind(), VcsKind::Jj);

        let _ = std::fs::remove_dir_all(&temp);
    }
}
