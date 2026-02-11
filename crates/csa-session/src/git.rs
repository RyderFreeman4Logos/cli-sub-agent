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
