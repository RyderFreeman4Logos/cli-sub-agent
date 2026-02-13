//! Git operations on the sessions repository.
//!
//! The sessions directory is tracked as a single git repository.
//! [`ensure_git_init`] must be called before any other git operation.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Ensure the sessions directory is a git repository. Initializes if needed.
pub fn ensure_git_init(sessions_dir: &Path) -> Result<()> {
    let git_dir = sessions_dir.join(".git");
    if git_dir.exists() {
        return Ok(());
    }

    std::fs::create_dir_all(sessions_dir)
        .with_context(|| format!("Failed to create sessions dir: {}", sessions_dir.display()))?;

    let output = Command::new("git")
        .args(["init"])
        .current_dir(sessions_dir)
        .output()
        .context("Failed to run git init")?;

    if !output.status.success() {
        anyhow::bail!(
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Configure git user for this repo (avoids "please tell me who you are" errors)
    match Command::new("git")
        .args(["config", "user.email", "csa@localhost"])
        .current_dir(sessions_dir)
        .output()
    {
        Ok(output) if !output.status.success() => {
            tracing::warn!(
                "git config user.email failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Err(e) => tracing::warn!("Failed to run git config user.email: {e}"),
        _ => {}
    }

    match Command::new("git")
        .args(["config", "user.name", "CSA Session"])
        .current_dir(sessions_dir)
        .output()
    {
        Ok(output) if !output.status.success() => {
            tracing::warn!(
                "git config user.name failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Err(e) => tracing::warn!("Failed to run git config user.name: {e}"),
        _ => {}
    }

    Ok(())
}

/// Stage and commit changes for a specific session directory.
/// Returns the short commit hash on success.
pub fn commit_session(sessions_dir: &Path, session_id: &str, message: &str) -> Result<String> {
    crate::validate::validate_session_id(session_id)?;
    ensure_git_init(sessions_dir)?;

    // Stage the session's files (use `--` to prevent option injection)
    let session_path = format!("{}/", session_id);
    let output = Command::new("git")
        .args(["add", "--", &session_path])
        .current_dir(sessions_dir)
        .output()
        .context("Failed to run git add")?;

    if !output.status.success() {
        anyhow::bail!(
            "git add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Check if there are staged changes for this session only.
    // git diff --cached --quiet exit codes: 0 = no diff, 1 = has diff, >1 = error
    let status = Command::new("git")
        .args(["diff", "--cached", "--quiet", "--", &session_path])
        .current_dir(sessions_dir)
        .output()
        .context("Failed to run git diff --cached")?;

    match status.status.code() {
        Some(0) => anyhow::bail!("No changes to commit for session '{session_id}'"),
        Some(1) => {} // Has staged changes, continue
        Some(code) => anyhow::bail!(
            "git diff --cached failed (exit {code}): {}",
            String::from_utf8_lossy(&status.stderr)
        ),
        None => anyhow::bail!("git diff --cached terminated by signal"),
    }

    // Commit
    let output = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(sessions_dir)
        .output()
        .context("Failed to run git commit")?;

    if !output.status.success() {
        anyhow::bail!(
            "git commit failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Return short hash
    let hash_output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(sessions_dir)
        .output()
        .context("Failed to get commit hash")?;

    if !hash_output.status.success() {
        anyhow::bail!(
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&hash_output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&hash_output.stdout)
        .trim()
        .to_string())
}

/// Get the git log for a session.
pub fn history(sessions_dir: &Path, session_id: &str) -> Result<String> {
    crate::validate::validate_session_id(session_id)?;

    if !sessions_dir.join(".git").exists() {
        anyhow::bail!("No git repository in sessions directory (run a session first)");
    }

    // Check if there are any commits (git log fails on empty repos)
    let head_check = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(sessions_dir)
        .output()
        .context("Failed to check HEAD")?;

    if !head_check.status.success() {
        return Ok(String::new()); // No commits yet
    }

    let session_path = format!("{}/", session_id);

    let output = Command::new("git")
        .args(["log", "--oneline", "--follow", "--", &session_path])
        .current_dir(sessions_dir)
        .output()
        .context("Failed to run git log")?;

    if !output.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ── ensure_git_init ────────────────────────────────────────────

    #[test]
    fn test_ensure_git_init_creates_repo() {
        let tmp = tempdir().expect("Failed to create temp dir");
        let sessions_dir = tmp.path().join("sessions");

        ensure_git_init(&sessions_dir).expect("First git init should succeed");
        assert!(sessions_dir.join(".git").exists());
    }

    #[test]
    fn test_ensure_git_init_idempotent() {
        let tmp = tempdir().expect("Failed to create temp dir");
        let sessions_dir = tmp.path().join("sessions");

        ensure_git_init(&sessions_dir).expect("First call should succeed");
        ensure_git_init(&sessions_dir).expect("Second call should also succeed (idempotent)");
        assert!(sessions_dir.join(".git").exists());
    }

    // ── commit_session ─────────────────────────────────────────────

    #[test]
    fn test_commit_session_returns_short_hash() {
        let tmp = tempdir().expect("Failed to create temp dir");
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        ensure_git_init(&sessions_dir).unwrap();

        // Create a valid session directory with a file
        let session_id = ulid::Ulid::new().to_string();
        let session_dir = sessions_dir.join(&session_id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("state.toml"), "placeholder = true").unwrap();

        let hash =
            commit_session(&sessions_dir, &session_id, "test commit").expect("Commit should work");
        assert!(!hash.is_empty(), "Should return a non-empty short hash");
        // Short git hashes are typically 7+ hex characters
        assert!(hash.len() >= 7, "Short hash should be at least 7 chars");
    }

    #[test]
    fn test_commit_session_no_changes_errors() {
        let tmp = tempdir().expect("Failed to create temp dir");
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        ensure_git_init(&sessions_dir).unwrap();

        // Create session with a file, commit it first
        let session_id = ulid::Ulid::new().to_string();
        let session_dir = sessions_dir.join(&session_id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("state.toml"), "placeholder = true").unwrap();
        commit_session(&sessions_dir, &session_id, "initial").unwrap();

        // Second commit without changes should fail
        let result = commit_session(&sessions_dir, &session_id, "no changes");
        assert!(result.is_err(), "Should error when there are no changes");
        assert!(
            result.unwrap_err().to_string().contains("No changes"),
            "Error message should mention no changes"
        );
    }

    // ── history ────────────────────────────────────────────────────

    #[test]
    fn test_history_after_commit() {
        let tmp = tempdir().expect("Failed to create temp dir");
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        ensure_git_init(&sessions_dir).unwrap();

        let session_id = ulid::Ulid::new().to_string();
        let session_dir = sessions_dir.join(&session_id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("state.toml"), "v = 1").unwrap();
        commit_session(&sessions_dir, &session_id, "first commit").unwrap();

        let log = history(&sessions_dir, &session_id).expect("History should work");
        assert!(
            log.contains("first commit"),
            "Log should contain the commit message"
        );
    }

    #[test]
    fn test_history_empty_repo_returns_empty_string() {
        let tmp = tempdir().expect("Failed to create temp dir");
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        ensure_git_init(&sessions_dir).unwrap();

        let session_id = ulid::Ulid::new().to_string();
        let log = history(&sessions_dir, &session_id).expect("History on empty repo should work");
        assert!(log.is_empty(), "Empty repo should return empty log");
    }

    #[test]
    fn test_history_no_git_repo_errors() {
        let tmp = tempdir().expect("Failed to create temp dir");
        let sessions_dir = tmp.path().join("sessions");
        // Do NOT init git

        let session_id = ulid::Ulid::new().to_string();
        let result = history(&sessions_dir, &session_id);
        assert!(
            result.is_err(),
            "Should error when no .git directory exists"
        );
    }
}
