//! Concrete VCS backend implementations for git and jj.
//!
//! # Security Model
//!
//! All commands use `std::process::Command` with `.args()` for argument passing,
//! which prevents shell injection by design. Arguments are passed as separate
//! OS strings, never through shell expansion. User-provided inputs (commit
//! messages, paths) are passed via `.args()` or `.arg()`, not string interpolation.
//! Template strings for jj `-T` flags are hardcoded constants.

use csa_core::vcs::{VcsBackend, VcsKind};
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
pub fn create_vcs_backend(project_root: &Path) -> Box<dyn VcsBackend> {
    match csa_core::vcs::detect_vcs_kind(project_root) {
        Some(VcsKind::Jj) => Box::new(JjBackend),
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
