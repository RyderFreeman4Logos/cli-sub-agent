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
/// Also ensures `.gitignore` excludes lock files (backfills for existing repos).
pub fn ensure_git_init(todos_dir: &Path) -> Result<()> {
    let git_dir = todos_dir.join(".git");
    if !git_dir.exists() {
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
    }

    // Ensure .gitignore excludes lock files (backfills for pre-existing repos)
    ensure_gitignore(todos_dir)?;

    Ok(())
}

/// Ensure `.gitignore` exists and contains `.lock` exclusion.
/// Creates the file if missing, or appends `.lock` if present but lacking it.
/// Commits the change as a bootstrap commit when newly created.
fn ensure_gitignore(todos_dir: &Path) -> Result<()> {
    let gitignore = todos_dir.join(".gitignore");
    if gitignore.exists() {
        let content = std::fs::read_to_string(&gitignore).context("Failed to read .gitignore")?;
        if content.lines().any(|l| l.trim() == ".lock") {
            return Ok(()); // already has .lock exclusion
        }
        // Append .lock exclusion to existing .gitignore
        let mut new_content = content;
        if !new_content.ends_with('\n') && !new_content.is_empty() {
            new_content.push('\n');
        }
        new_content.push_str(".lock\n");
        std::fs::write(&gitignore, new_content).context("Failed to update .gitignore")?;
    } else {
        std::fs::write(&gitignore, ".lock\n").context("Failed to write .gitignore")?;
    }

    // Stage and commit the .gitignore change (no-op if already committed)
    let _ = Command::new("git")
        .args(["add", "--", ".gitignore"])
        .current_dir(todos_dir)
        .output();
    // Pathspec `-- .gitignore` prevents committing unrelated pre-staged files.
    let _ = Command::new("git")
        .args([
            "commit",
            "-m",
            "bootstrap: add .gitignore",
            "--",
            ".gitignore",
        ])
        .current_dir(todos_dir)
        .output();

    Ok(())
}

/// Stage and commit ALL pending changes in the todos repository.
///
/// Stages everything (`git add -A`) so that modifications across multiple
/// plan directories, metadata updates, and any other tracked files are
/// committed together.  The todos repo only contains TODO.md files and
/// `metadata.toml` -- no sensitive data -- so a blanket add is safe.
///
/// The `timestamp` parameter identifies the plan that triggered the save
/// (used for validation only).  Use [`save_file`] when you need to commit
/// a single file without touching anything else.
///
/// Returns the short commit hash, or `None` if there were no changes to commit.
pub fn save(todos_dir: &Path, timestamp: &str, message: &str) -> Result<Option<String>> {
    crate::validate_timestamp(timestamp)?;
    ensure_git_init(todos_dir)?;

    // Stage ALL pending changes (additions, modifications, deletions)
    let output = Command::new("git")
        .args(["add", "-A"])
        .current_dir(todos_dir)
        .output()
        .context("Failed to run git add")?;

    if !output.status.success() {
        anyhow::bail!(
            "git add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Check for any staged changes
    let status = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(todos_dir)
        .output()
        .context("Failed to run git diff --cached")?;

    match status.status.code() {
        Some(0) => return Ok(None),
        Some(1) => {}
        Some(code) => anyhow::bail!(
            "git diff --cached failed (exit {}): {}",
            code,
            String::from_utf8_lossy(&status.stderr)
        ),
        None => anyhow::bail!("git diff --cached terminated by signal"),
    }

    // Commit all staged changes
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

    Ok(Some(
        String::from_utf8_lossy(&hash_output.stdout)
            .trim()
            .to_string(),
    ))
}

/// Stage and commit specific files within a plan directory.
///
/// `files` are paths relative to `todos_dir` (e.g. `["20260211T023000/metadata.toml"]`).
/// Returns the short commit hash, or `None` if there were no changes to commit.
pub fn save_file(
    todos_dir: &Path,
    timestamp: &str,
    file: &str,
    message: &str,
) -> Result<Option<String>> {
    save_paths(todos_dir, timestamp, &[file], message)
}

/// Internal: stage given paths and commit.
fn save_paths(
    todos_dir: &Path,
    timestamp: &str,
    paths: &[&str],
    message: &str,
) -> Result<Option<String>> {
    crate::validate_timestamp(timestamp)?;
    ensure_git_init(todos_dir)?;

    // Stage the specified paths (use `--` to prevent option injection)
    let mut args: Vec<&str> = vec!["add", "--"];
    args.extend(paths.iter().copied());
    let output = Command::new("git")
        .args(&args)
        .current_dir(todos_dir)
        .output()
        .context("Failed to run git add")?;

    if !output.status.success() {
        anyhow::bail!(
            "git add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Check if there are staged changes for the specified paths only.
    // git diff --cached --quiet exit codes: 0 = no diff, 1 = has diff, >1 = error
    let mut diff_args: Vec<&str> = vec!["diff", "--cached", "--quiet", "--"];
    diff_args.extend(paths.iter().copied());
    let status = Command::new("git")
        .args(&diff_args)
        .current_dir(todos_dir)
        .output()
        .context("Failed to run git diff --cached")?;

    match status.status.code() {
        Some(0) => return Ok(None), // No changes
        Some(1) => {}               // Has staged changes, continue
        Some(code) => anyhow::bail!(
            "git diff --cached failed (exit {}): {}",
            code,
            String::from_utf8_lossy(&status.stderr)
        ),
        None => anyhow::bail!("git diff --cached terminated by signal"),
    }

    // Commit only the specified paths (not other staged changes)
    let mut commit_args: Vec<&str> = vec!["commit", "-m", message, "--"];
    commit_args.extend(paths.iter().copied());
    let output = Command::new("git")
        .args(&commit_args)
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

    Ok(Some(
        String::from_utf8_lossy(&hash_output.stdout)
            .trim()
            .to_string(),
    ))
}

/// Get the diff of a plan's TODO.md against a revision.
///
/// When `revision` is `None`:
/// - if working copy has uncommitted TODO.md changes, show working copy diff
/// - if working copy is clean and there are >=2 committed versions, show v2 -> v1
/// - if working copy is clean and there is 1 committed version, show initial content as new
/// - if file has never been committed, show the entire working file as new content
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
                    return new_file_diff_from_working_copy(todos_dir, &file_path);
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

    let working_diff = String::from_utf8_lossy(&output.stdout).to_string();
    if revision.is_some() || !working_diff.is_empty() {
        return Ok(working_diff);
    }

    // Working copy is clean: show most recent saved changes by default.
    let versions = list_versions(todos_dir, timestamp)?;
    match versions.len() {
        0 => Ok(String::new()),
        1 => new_file_diff_from_commit(todos_dir, &versions[0], &file_path),
        _ => diff_versions(todos_dir, timestamp, 2, 1),
    }
}

fn new_file_diff_from_working_copy(todos_dir: &Path, file_path: &str) -> Result<String> {
    let full_path = todos_dir.join(file_path);
    if !full_path.exists() {
        return Ok(String::new());
    }

    let content = std::fs::read_to_string(&full_path)
        .with_context(|| format!("Failed to read {}", full_path.display()))?;
    Ok(render_new_file_diff(file_path, &content))
}

fn new_file_diff_from_commit(todos_dir: &Path, hash: &str, file_path: &str) -> Result<String> {
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

    let content = String::from_utf8_lossy(&output.stdout);
    Ok(render_new_file_diff(file_path, &content))
}

fn render_new_file_diff(file_path: &str, content: &str) -> String {
    format!(
        "--- /dev/null\n+++ b/{file_path}\n@@ -0,0 +1,{} @@\n{}",
        content.lines().count(),
        content
            .lines()
            .map(|line| format!("+{line}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
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

    let plan_prefix = format!("{}/", timestamp);

    let output = Command::new("git")
        .args(["log", "--oneline", "--", &plan_prefix])
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

    // Track TODO.md specifically — versions correspond to document changes,
    // not metadata-only commits (which show in `history` instead).
    let file_path = format!("{}/TODO.md", timestamp);

    let output = Command::new("git")
        .args(["log", "--format=%H", "--", &file_path])
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Helper: create a minimal todos repo with a plan directory.
    fn setup_todos_dir(dir: &Path, ts: &str) {
        let plan_dir = dir.join(ts);
        fs::create_dir_all(&plan_dir).unwrap();
        fs::write(plan_dir.join("TODO.md"), "# Test\n").unwrap();
        fs::write(
            plan_dir.join("metadata.toml"),
            "title = \"test\"\nstatus = \"draft\"\n",
        )
        .unwrap();
        ensure_git_init(dir).unwrap();
    }

    #[test]
    fn test_save_commits_all_pending_changes() {
        let dir = tempdir().unwrap();
        let todos = dir.path();
        let ts_a = "20260101T000000";
        let ts_b = "20260102T000000";

        // Create two plans
        setup_todos_dir(todos, ts_a);
        let plan_b_dir = todos.join(ts_b);
        fs::create_dir_all(&plan_b_dir).unwrap();
        fs::write(plan_b_dir.join("TODO.md"), "# Plan B\n").unwrap();

        // Save via plan A — should commit ALL pending changes including plan B
        let hash = save(todos, ts_a, "save all").unwrap();
        assert!(hash.is_some(), "should have committed changes");

        // Git status should be completely clean
        let status = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(todos)
            .output()
            .unwrap();
        let status_str = String::from_utf8_lossy(&status.stdout);
        assert!(
            status_str.trim().is_empty(),
            "git status should be clean after save, got: {status_str}"
        );
    }

    #[test]
    fn test_save_returns_none_when_clean() {
        let dir = tempdir().unwrap();
        let todos = dir.path();
        let ts = "20260101T000000";

        setup_todos_dir(todos, ts);
        // First save commits everything
        save(todos, ts, "initial").unwrap();
        // Second save with no changes should return None
        let hash = save(todos, ts, "no-op").unwrap();
        assert!(hash.is_none(), "should return None when nothing to commit");
    }

    #[test]
    fn test_gitignore_excludes_lock_file() {
        let dir = tempdir().unwrap();
        let todos = dir.path();
        let ts = "20260101T000000";

        setup_todos_dir(todos, ts);
        // Create a .lock file (used by TodoManager for flock)
        fs::write(todos.join(".lock"), "").unwrap();

        let hash = save(todos, ts, "initial").unwrap();
        assert!(hash.is_some());

        // .lock should NOT appear in committed files
        let output = Command::new("git")
            .args(["ls-files"])
            .current_dir(todos)
            .output()
            .unwrap();
        let files = String::from_utf8_lossy(&output.stdout);
        assert!(
            !files.contains(".lock"),
            ".lock should be excluded by .gitignore, tracked files: {files}"
        );
    }

    #[test]
    fn test_gitignore_backfill_for_existing_repo() {
        let dir = tempdir().unwrap();
        let todos = dir.path();

        // Simulate a pre-existing repo WITHOUT .gitignore
        fs::create_dir_all(todos).unwrap();
        let output = Command::new("git")
            .args(["init"])
            .current_dir(todos)
            .output()
            .unwrap();
        assert!(output.status.success());
        let _ = Command::new("git")
            .args(["config", "user.email", "test@test"])
            .current_dir(todos)
            .output();
        let _ = Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(todos)
            .output();

        // .git exists but NO .gitignore
        assert!(todos.join(".git").exists());
        assert!(!todos.join(".gitignore").exists());

        // Call ensure_git_init — should backfill .gitignore
        ensure_git_init(todos).unwrap();

        // .gitignore should now exist and contain .lock
        let gitignore = fs::read_to_string(todos.join(".gitignore")).unwrap();
        assert!(
            gitignore.contains(".lock"),
            ".gitignore should contain .lock after backfill, got: {gitignore}"
        );
    }

    #[test]
    fn test_gitignore_bootstrap_does_not_commit_unrelated_staged_files() {
        let dir = tempdir().unwrap();
        let todos = dir.path();

        // Set up pre-existing repo WITHOUT .gitignore
        fs::create_dir_all(todos).unwrap();
        let output = Command::new("git")
            .args(["init"])
            .current_dir(todos)
            .output()
            .unwrap();
        assert!(output.status.success());
        let _ = Command::new("git")
            .args(["config", "user.email", "test@test"])
            .current_dir(todos)
            .output();
        let _ = Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(todos)
            .output();

        // Create and stage an unrelated plan file BEFORE bootstrap
        let plan_dir = todos.join("20260101T000000");
        fs::create_dir_all(&plan_dir).unwrap();
        fs::write(plan_dir.join("TODO.md"), "# Plan A").unwrap();
        let _ = Command::new("git")
            .args(["add", "20260101T000000/TODO.md"])
            .current_dir(todos)
            .output();

        // Trigger bootstrap via ensure_git_init
        ensure_git_init(todos).unwrap();

        // Bootstrap commit should contain ONLY .gitignore, not the staged plan
        let log_output = Command::new("git")
            .args(["show", "--name-only", "--pretty=format:", "HEAD"])
            .current_dir(todos)
            .output()
            .unwrap();
        let committed_files = String::from_utf8_lossy(&log_output.stdout);
        let files: Vec<&str> = committed_files.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(
            files,
            vec![".gitignore"],
            "Bootstrap commit must only contain .gitignore, got: {files:?}"
        );

        // The unrelated plan file should still be staged (not committed)
        let status = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(todos)
            .output()
            .unwrap();
        let status_str = String::from_utf8_lossy(&status.stdout);
        assert!(
            status_str.contains("A  20260101T000000/TODO.md"),
            "Plan file should remain staged after bootstrap, got: {status_str}"
        );
    }

    #[test]
    fn test_save_file_only_commits_specified_file() {
        let dir = tempdir().unwrap();
        let todos = dir.path();
        let ts = "20260101T000000";

        setup_todos_dir(todos, ts);
        // Initial commit of everything
        save(todos, ts, "initial").unwrap();

        // Modify two files
        fs::write(todos.join(ts).join("TODO.md"), "# Updated\n").unwrap();
        fs::write(
            todos.join(ts).join("metadata.toml"),
            "title = \"updated\"\nstatus = \"approved\"\n",
        )
        .unwrap();

        // save_file only the metadata — TODO.md should remain dirty
        let file_path = format!("{}/metadata.toml", ts);
        save_file(todos, ts, &file_path, "metadata only").unwrap();

        let status = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(todos)
            .output()
            .unwrap();
        let status_str = String::from_utf8_lossy(&status.stdout);
        assert!(
            status_str.contains("TODO.md"),
            "TODO.md should still be dirty after save_file on metadata only, got: {status_str}"
        );
    }

    // Extended tests in git_ext_tests.rs
    include!("git_ext_tests.rs");
}
