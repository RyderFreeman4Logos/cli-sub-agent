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

/// Get the diff of a plan's TODO.md against a revision.
///
/// When `revision` is `None`, diffs against the file's own last commit
/// (not HEAD, which may belong to a different plan in this multi-plan repo).
/// If the file has never been committed, shows the entire file as new content.
pub fn diff(todos_dir: &Path, timestamp: &str, revision: Option<&str>) -> Result<String> {
    crate::validate_timestamp(timestamp)?;

    // Check if git repo exists
    if !todos_dir.join(".git").exists() {
        anyhow::bail!("No git repository in todos directory (run `csa todo save` first)");
    }

    let file_path = format!("{}/TODO.md", timestamp);

    let rev = match revision {
        Some(r) => {
            validate_revision(r)?;
            r.to_string()
        }
        None => {
            // Find the file's own last commit (not HEAD which may be another plan's)
            match file_last_commit(todos_dir, &file_path)? {
                Some(hash) => hash,
                None => {
                    // File never committed — show full working copy as new
                    let full_path = todos_dir.join(&file_path);
                    return if full_path.exists() {
                        let content = std::fs::read_to_string(&full_path)
                            .with_context(|| format!("Failed to read {}", full_path.display()))?;
                        Ok(format!(
                            "--- /dev/null\n+++ b/{file_path}\n@@ -0,0 +1,{} @@\n{}",
                            content.lines().count(),
                            content
                                .lines()
                                .map(|l| format!("+{l}"))
                                .collect::<Vec<_>>()
                                .join("\n")
                        ))
                    } else {
                        Ok(String::new())
                    };
                }
            }
        }
    };

    let output = Command::new("git")
        .args(["diff", &rev, "--", &file_path])
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

/// Find the last commit hash that touched a specific file.
///
/// Returns `None` if the file has never been committed.
fn file_last_commit(todos_dir: &Path, file_path: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%H", "--", file_path])
        .current_dir(todos_dir)
        .output()
        .context("Failed to run git log")?;

    match output.status.code() {
        Some(0) => {}
        // Exit 128 = unborn branch (no commits yet) — treat as "never committed"
        Some(128) => return Ok(None),
        Some(code) => anyhow::bail!(
            "git log failed (exit {code}): {}",
            String::from_utf8_lossy(&output.stderr)
        ),
        None => anyhow::bail!("git log terminated by signal"),
    }

    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(if hash.is_empty() { None } else { Some(hash) })
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

/// List commit hashes for a plan's TODO.md (newest first).
///
/// Version 0 = current working copy (not in this list).
/// Version 1 = last committed, version 2 = second-to-last, etc.
pub fn list_versions(todos_dir: &Path, timestamp: &str) -> Result<Vec<String>> {
    crate::validate_timestamp(timestamp)?;

    if !todos_dir.join(".git").exists() {
        return Ok(Vec::new());
    }

    let file_path = format!("{}/TODO.md", timestamp);

    let output = Command::new("git")
        .args(["log", "--format=%H", "--follow", "--", &file_path])
        .current_dir(todos_dir)
        .output()
        .context("Failed to run git log")?;

    if !output.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect())
}

/// Show a specific historical version of a plan's TODO.md.
///
/// `version` is 1-indexed: 1 = last committed version, 2 = the one before, etc.
/// Returns the file content at that version.
pub fn show_version(todos_dir: &Path, timestamp: &str, version: usize) -> Result<String> {
    if version == 0 {
        anyhow::bail!("Version 0 is the current working copy — use `show` without --version");
    }

    let versions = list_versions(todos_dir, timestamp)?;
    if versions.is_empty() {
        anyhow::bail!("No committed versions found for plan '{timestamp}'");
    }

    let idx = version - 1;
    if idx >= versions.len() {
        anyhow::bail!(
            "Version {version} does not exist. This plan has {} committed version(s)",
            versions.len()
        );
    }

    let hash = &versions[idx];
    let file_path = format!("{}/TODO.md", timestamp);

    let output = Command::new("git")
        .args(["show", &format!("{hash}:{file_path}")])
        .current_dir(todos_dir)
        .output()
        .context("Failed to run git show")?;

    if !output.status.success() {
        anyhow::bail!(
            "git show failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Diff between two historical versions of a plan's TODO.md.
///
/// `from_version` and `to_version` are 1-indexed (1 = last committed).
/// Diffs from older (`from_version`) to newer (`to_version`).
pub fn diff_versions(
    todos_dir: &Path,
    timestamp: &str,
    from_version: usize,
    to_version: usize,
) -> Result<String> {
    let versions = list_versions(todos_dir, timestamp)?;
    if versions.is_empty() {
        anyhow::bail!("No committed versions found for plan '{timestamp}'");
    }

    for (label, v) in [("from", from_version), ("to", to_version)] {
        if v == 0 || v > versions.len() {
            anyhow::bail!(
                "--{label} version {v} does not exist. This plan has {} committed version(s)",
                versions.len()
            );
        }
    }

    let from_hash = &versions[from_version - 1];
    let to_hash = &versions[to_version - 1];
    let file_path = format!("{}/TODO.md", timestamp);

    let output = Command::new("git")
        .args(["diff", from_hash, to_hash, "--", &file_path])
        .current_dir(todos_dir)
        .output()
        .context("Failed to run git diff")?;

    if !output.status.success() {
        anyhow::bail!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
