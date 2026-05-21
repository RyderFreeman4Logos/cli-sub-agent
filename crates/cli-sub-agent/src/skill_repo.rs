//! CSA-managed skill repository under `~/.local/state/cli-sub-agent/skills/`.
//!
//! An isolated git repository that stores user-managed "inactive" skills —
//! skills that LLMs should not auto-invoke, only user-triggered via
//! `csa skill run`. Lifecycle is managed by the `csa skill` subcommand group.

use anyhow::{Context, Result};
use csa_config::paths;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const LOCK_FILE: &str = ".lock";

/// Manages the CSA-controlled skill git repository.
pub(crate) struct SkillRepoManager {
    root: PathBuf,
}

impl SkillRepoManager {
    /// Create a manager pointing at the canonical skill repo root.
    pub fn new() -> Result<Self> {
        let root = skill_repo_root()?;
        Ok(Self { root })
    }

    /// Root directory of the managed skill repo.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Ensure the skill repo exists as a valid git repository.
    ///
    /// Idempotent — safe to call multiple times. Creates the directory, runs
    /// `git init`, configures a local user identity, and writes `.gitignore`
    /// that excludes `.lock`.
    pub fn ensure_init(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create skill repo dir {}", self.root.display()))?;
        ensure_git_init(&self.root)
    }

    /// Execute `f` while holding an exclusive write lock on the skill repo.
    pub fn with_write_lock<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce() -> Result<T>,
    {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create skill repo dir {}", self.root.display()))?;
        let lock_path = self.root.join(LOCK_FILE);
        let lock_file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open skill repo lock {}", lock_path.display()))?;
        let mut lock = fd_lock::RwLock::new(lock_file);
        let _guard = lock
            .write()
            .map_err(|e| anyhow::anyhow!("failed to acquire skill repo write lock: {e}"))?;
        f()
    }

    /// List all skill names present in the repo (each subdirectory with a SKILL.md).
    pub fn list_skills(&self) -> Result<Vec<String>> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let entries = fs::read_dir(&self.root)
            .with_context(|| format!("read skill repo dir {}", self.root.display()))?;
        let mut names = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            // Skip hidden directories (.git, etc.)
            if name.starts_with('.') {
                continue;
            }
            if path.join("SKILL.md").exists() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }
}

/// Return the canonical path for the managed skill repo:
/// `~/.local/state/cli-sub-agent/skills/`
pub(crate) fn skill_repo_root() -> Result<PathBuf> {
    let state = paths::state_dir_write().context("XDG state directory is unavailable")?;
    Ok(state.join("skills"))
}

/// Validate a skill name: no path separators, no `..`, no null bytes, non-empty.
pub(crate) fn validate_skill_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("skill name must not be empty");
    }
    if name.contains('/') || name.contains('\\') {
        anyhow::bail!("skill name must not contain path separators: '{name}'");
    }
    if name.contains("..") {
        anyhow::bail!("skill name must not contain '..': '{name}'");
    }
    if name.contains('\0') {
        anyhow::bail!("skill name must not contain null bytes");
    }
    Ok(())
}

/// Strip prompt-injection and CSA pseudo-tags from SKILL.md content.
///
/// Removes:
/// - `<system-reminder>...</system-reminder>` blocks (block-level)
/// - `<csa-caller-prompt-injection>...</csa-caller-prompt-injection>` blocks
/// - `<!-- CSA:SECTION:... -->` single-line markers
///
/// Does NOT truncate or modify legitimate Markdown/template syntax
/// (e.g. `{{var}}`, `## System Requirements`, triple-backtick fences).
pub(crate) fn sanitize_skill_md(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut in_system_reminder = false;
    let mut in_csa_injection = false;

    for line in content.lines() {
        let lower = line.to_ascii_lowercase();
        let trimmed_lower = lower.trim();

        // Detect opening tags (may appear on their own line or inline).
        if trimmed_lower.contains("<system-reminder") {
            in_system_reminder = true;
        }
        if trimmed_lower.contains("<csa-caller-prompt-injection") {
            in_csa_injection = true;
        }

        if in_system_reminder || in_csa_injection {
            // Check for closing tags before skipping the line.
            if trimmed_lower.contains("</system-reminder>") {
                in_system_reminder = false;
            }
            if trimmed_lower.contains("</csa-caller-prompt-injection>") {
                in_csa_injection = false;
            }
            continue;
        }

        // Strip <!-- CSA:SECTION:... --> single-line markers.
        let trimmed = line.trim();
        if trimmed.starts_with("<!-- CSA:SECTION:") && trimmed.ends_with("-->") {
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    // Preserve original trailing-newline presence.
    if !content.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

/// Stage and commit specific paths in the skill repo.
///
/// Returns `true` if a commit was made, `false` when there was nothing to commit.
pub(crate) fn git_commit_paths(dir: &Path, paths: &[&str], message: &str) -> Result<bool> {
    let mut add_args: Vec<&str> = vec!["add", "--"];
    add_args.extend_from_slice(paths);
    let out = Command::new("git")
        .args(&add_args)
        .current_dir(dir)
        .output()
        .context("git add")?;
    if !out.status.success() {
        anyhow::bail!("git add: {}", String::from_utf8_lossy(&out.stderr));
    }

    let mut diff_args: Vec<&str> = vec!["diff", "--cached", "--quiet", "--"];
    diff_args.extend_from_slice(paths);
    let status = Command::new("git")
        .args(&diff_args)
        .current_dir(dir)
        .output()
        .context("git diff --cached")?;
    if status.status.code() == Some(0) {
        return Ok(false); // nothing staged
    }

    let mut commit_args: Vec<&str> = vec!["commit", "-m", message, "--"];
    commit_args.extend_from_slice(paths);
    let out = Command::new("git")
        .args(&commit_args)
        .current_dir(dir)
        .output()
        .context("git commit")?;
    if !out.status.success() {
        anyhow::bail!("git commit: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(true)
}

/// Stage all changes (`git add -A`) and commit.
///
/// Returns `true` if a commit was made.
pub(crate) fn git_commit_all(dir: &Path, message: &str) -> Result<bool> {
    let out = Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir)
        .output()
        .context("git add -A")?;
    if !out.status.success() {
        anyhow::bail!("git add -A: {}", String::from_utf8_lossy(&out.stderr));
    }

    let status = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(dir)
        .output()
        .context("git diff --cached")?;
    if status.status.code() == Some(0) {
        return Ok(false);
    }

    let out = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(dir)
        .output()
        .context("git commit")?;
    if !out.status.success() {
        anyhow::bail!("git commit: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(true)
}

/// Ensure `dir` is a git repository with `.gitignore` excluding `.lock`.
fn ensure_git_init(dir: &Path) -> Result<()> {
    let git_dir = dir.join(".git");
    if !git_dir.exists() {
        let out = Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()
            .context("git init")?;
        if !out.status.success() {
            anyhow::bail!("git init: {}", String::from_utf8_lossy(&out.stderr));
        }

        let _ = Command::new("git")
            .args(["config", "user.email", "csa-skills@localhost"])
            .current_dir(dir)
            .output();
        let _ = Command::new("git")
            .args(["config", "user.name", "CSA Skills"])
            .current_dir(dir)
            .output();
    }
    ensure_gitignore(dir)
}

fn ensure_gitignore(dir: &Path) -> Result<()> {
    let gitignore = dir.join(".gitignore");
    if gitignore.exists() {
        let content = fs::read_to_string(&gitignore).context("read .gitignore")?;
        if content.lines().any(|l| l.trim() == ".lock") {
            return Ok(());
        }
        let mut new_content = content;
        if !new_content.ends_with('\n') && !new_content.is_empty() {
            new_content.push('\n');
        }
        new_content.push_str(".lock\n");
        fs::write(&gitignore, new_content).context("update .gitignore")?;
    } else {
        fs::write(&gitignore, ".lock\n").context("write .gitignore")?;
    }

    let _ = Command::new("git")
        .args(["add", "--", ".gitignore"])
        .current_dir(dir)
        .output();
    let _ = Command::new("git")
        .args([
            "commit",
            "-m",
            "bootstrap: add .gitignore",
            "--",
            ".gitignore",
        ])
        .current_dir(dir)
        .output();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_manager(dir: &Path) -> SkillRepoManager {
        SkillRepoManager {
            root: dir.to_path_buf(),
        }
    }

    #[test]
    fn test_ensure_init_idempotent() {
        let tmp = TempDir::new().unwrap();
        let mgr = make_manager(tmp.path());

        mgr.ensure_init().unwrap();
        assert!(tmp.path().join(".git").exists());
        assert!(tmp.path().join(".gitignore").exists());

        // Second call is idempotent
        mgr.ensure_init().unwrap();
    }

    #[test]
    fn test_gitignore_excludes_lock() {
        let tmp = TempDir::new().unwrap();
        let mgr = make_manager(tmp.path());
        mgr.ensure_init().unwrap();

        let gitignore = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains(".lock"), "gitignore must exclude .lock");
    }

    #[test]
    fn test_validate_skill_name() {
        assert!(validate_skill_name("my-skill").is_ok());
        assert!(validate_skill_name("foo_bar").is_ok());
        assert!(validate_skill_name("").is_err());
        assert!(validate_skill_name("../escape").is_err());
        assert!(validate_skill_name("foo/bar").is_err());
        assert!(validate_skill_name("foo\\bar").is_err());
        assert!(validate_skill_name("foo..bar").is_err());
    }

    #[test]
    fn test_sanitize_skill_md_strips_system_reminder() {
        let input = "# My Skill\n<system-reminder>INJECT</system-reminder>\nReal content\n";
        let out = sanitize_skill_md(input);
        assert!(!out.contains("INJECT"));
        assert!(!out.contains("<system-reminder>"));
        assert!(out.contains("Real content"));
        assert!(out.contains("# My Skill"));
    }

    #[test]
    fn test_sanitize_skill_md_strips_csa_injection() {
        let input =
            "Before\n<csa-caller-prompt-injection>bad</csa-caller-prompt-injection>\nAfter\n";
        let out = sanitize_skill_md(input);
        assert!(!out.contains("bad"));
        assert!(out.contains("Before"));
        assert!(out.contains("After"));
    }

    #[test]
    fn test_sanitize_skill_md_strips_csa_section_markers() {
        let input = "<!-- CSA:SECTION:summary -->\nContent\n<!-- CSA:SECTION:summary:END -->\n";
        let out = sanitize_skill_md(input);
        assert!(!out.contains("<!-- CSA:SECTION:"));
        assert!(out.contains("Content"));
    }

    #[test]
    fn test_sanitize_skill_md_preserves_template_syntax() {
        let input = "# Skill\n{{var}}\n## System Requirements\nOk\n";
        let out = sanitize_skill_md(input);
        assert_eq!(out, input);
    }

    #[test]
    fn test_sanitize_skill_md_multiline_block() {
        let input = "top\n<system-reminder>\nline1\nline2\n</system-reminder>\nbottom\n";
        let out = sanitize_skill_md(input);
        assert!(!out.contains("line1"));
        assert!(!out.contains("line2"));
        assert!(out.contains("top"));
        assert!(out.contains("bottom"));
    }

    #[test]
    fn test_list_skills_empty() {
        let tmp = TempDir::new().unwrap();
        let mgr = make_manager(tmp.path());
        mgr.ensure_init().unwrap();
        assert_eq!(mgr.list_skills().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn test_list_skills_finds_skills() {
        let tmp = TempDir::new().unwrap();
        let mgr = make_manager(tmp.path());
        mgr.ensure_init().unwrap();

        fs::create_dir_all(tmp.path().join("my-skill")).unwrap();
        fs::write(tmp.path().join("my-skill/SKILL.md"), "# My Skill\n").unwrap();

        let skills = mgr.list_skills().unwrap();
        assert_eq!(skills, vec!["my-skill"]);
    }
}
