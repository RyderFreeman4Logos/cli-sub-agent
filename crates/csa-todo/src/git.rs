//! Git operations on the todos repository.
//!
//! The todos directory is tracked as a single git repository.
//! [`ensure_git_init`] must be called before any other git operation.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Validate a revision spec to prevent option injection.
///
/// Rejects strings starting with `-` (would be parsed as git options).
fn validate_revision(rev: &str) -> Result<()> {
    if rev.starts_with('-') {
        anyhow::bail!("Invalid revision: '{rev}' (must not start with '-')");
    }
    Ok(())
}

/// Ensure the todos directory is a git repository. Initializes if needed.
pub fn ensure_git_init(todos_dir: &Path) -> Result<()> {
    let git_dir = todos_dir.join(".git");
    if git_dir.exists() {
        return Ok(());
    }

    std::fs::create_dir_all(todos_dir)
        .with_context(|| format!("Failed to create todos dir: {}", todos_dir.display()))?;

    let output = Command::new("git")
        .args(["init"])
        .current_dir(todos_dir)
        .output()
        .context("Failed to run git init")?;

    if !output.status.success() {
        anyhow::bail!(
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Configure git user for this repo (avoids "please tell me who you are" errors)
    let email_result = Command::new("git")
        .args(["config", "user.email", "csa@localhost"])
        .current_dir(todos_dir)
        .output();
    if let Err(e) = &email_result {
        tracing::warn!("Failed to set git user.email: {e}");
    }

    let name_result = Command::new("git")
        .args(["config", "user.name", "CSA Todo"])
        .current_dir(todos_dir)
        .output();
    if let Err(e) = &name_result {
        tracing::warn!("Failed to set git user.name: {e}");
    }

    Ok(())
}

/// Stage and commit changes for a specific plan directory.
///
/// Returns the short commit hash on success.
pub fn save(todos_dir: &Path, timestamp: &str, message: &str) -> Result<String> {
    crate::validate_timestamp(timestamp)?;
    ensure_git_init(todos_dir)?;

    // Stage the plan's files (use `--` to prevent option injection)
    let plan_path = format!("{}/", timestamp);
    let output = Command::new("git")
        .args(["add", "--", &plan_path])
        .current_dir(todos_dir)
        .output()
        .context("Failed to run git add")?;

    if !output.status.success() {
        anyhow::bail!(
            "git add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Check if there are staged changes.
    // git diff --cached --quiet exit codes: 0 = no diff, 1 = has diff, >1 = error
    let status = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(todos_dir)
        .output()
        .context("Failed to run git diff --cached")?;

    match status.status.code() {
        Some(0) => anyhow::bail!("No changes to save for plan '{timestamp}'"),
        Some(1) => {} // Has staged changes, continue
        Some(code) => anyhow::bail!(
            "git diff --cached failed (exit {}): {}",
            code,
            String::from_utf8_lossy(&status.stderr)
        ),
        None => anyhow::bail!("git diff --cached terminated by signal"),
    }

    // Commit
    let output = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(todos_dir)
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
        .current_dir(todos_dir)
        .output()
        .context("Failed to get commit hash")?;

    Ok(String::from_utf8_lossy(&hash_output.stdout)
        .trim()
        .to_string())
}

/// Get the diff of a plan's TODO.md against a revision (default: HEAD).
pub fn diff(todos_dir: &Path, timestamp: &str, revision: Option<&str>) -> Result<String> {
    crate::validate_timestamp(timestamp)?;
    let rev = revision.unwrap_or("HEAD");
    validate_revision(rev)?;

    let file_path = format!("{}/TODO.md", timestamp);

    // Check if git repo exists
    if !todos_dir.join(".git").exists() {
        anyhow::bail!("No git repository in todos directory (run `csa todo save` first)");
    }

    let output = Command::new("git")
        .args(["diff", rev, "--", &file_path])
        .current_dir(todos_dir)
        .output()
        .context("Failed to run git diff")?;

    if !output.status.success() {
        anyhow::bail!(
            "git diff failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Get the git log for a plan's TODO.md.
pub fn history(todos_dir: &Path, timestamp: &str) -> Result<String> {
    crate::validate_timestamp(timestamp)?;

    if !todos_dir.join(".git").exists() {
        anyhow::bail!("No git repository in todos directory (run `csa todo save` first)");
    }

    let file_path = format!("{}/TODO.md", timestamp);

    let output = Command::new("git")
        .args(["log", "--oneline", "--follow", "--", &file_path])
        .current_dir(todos_dir)
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
