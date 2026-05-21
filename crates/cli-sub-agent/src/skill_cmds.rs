use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use tracing::info;

use crate::skill_repo::{
    SkillRepoManager, git_commit_all, git_commit_paths, skill_repo_root, validate_skill_name,
};

/// Handle skill install command
pub(crate) fn handle_skill_install(source: String, target: Option<String>) -> Result<()> {
    // 1. Parse source to get user/repo
    let repo = parse_github_source(&source)?;
    eprintln!("Installing skills from: {repo}");

    // 2. Clone repo to temp directory
    let temp_dir = TempDir::new().context("Failed to create temporary directory")?;
    clone_repository(&repo, temp_dir.path())?;

    // 3. Find skills directory in cloned repo
    let skills_source_dir = find_skills_directory(temp_dir.path())?;
    eprintln!("Found skills directory: {}", skills_source_dir.display());

    // 4. Determine target directory
    let target_dir = determine_target_directory(target.as_deref())?;
    eprintln!("Installing to: {}", target_dir.display());

    // 5. Create target directory if it doesn't exist
    fs::create_dir_all(&target_dir).context("Failed to create target directory")?;

    // 6. Copy skill directories to target
    let installed = copy_skills(&skills_source_dir, &target_dir)?;

    if installed.is_empty() {
        eprintln!("No skills found to install.");
    } else {
        eprintln!("\nInstalled {} skill(s):", installed.len());
        for skill in installed {
            eprintln!("  - {skill}");
        }
    }

    Ok(())
}

/// Handle skill list command — shows both active (.claude/skills/) and managed (state dir) skills.
pub(crate) fn handle_skill_list() -> Result<()> {
    let mut any = false;

    // --- Active skills (project + global .claude/skills/) ---
    let active_dirs = collect_active_skill_dirs();
    let mut active_skills: Vec<(String, PathBuf)> = Vec::new();
    for dir in &active_dirs {
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(dir)?.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) if !n.starts_with('.') => n.to_string(),
                _ => continue,
            };
            if path.join("SKILL.md").exists() {
                active_skills.push((name, path));
            }
        }
    }
    active_skills.sort_by(|a, b| a.0.cmp(&b.0));
    active_skills.dedup_by(|a, b| a.0 == b.0);

    if !active_skills.is_empty() {
        eprintln!("Active skills (.claude/skills/):\n");
        for (name, path) in &active_skills {
            let title = read_skill_title(path).unwrap_or_else(|| name.clone());
            println!("  {name} [active] - {title}");
        }
        any = true;
    }

    // --- Managed skills (CSA state dir) ---
    let mgr = SkillRepoManager::new()?;
    let managed = mgr.list_skills()?;
    if !managed.is_empty() {
        if any {
            eprintln!();
        }
        eprintln!("Managed skills (csa skill repo):\n");
        for name in &managed {
            let path = mgr.root().join(name);
            let title = read_skill_title(&path).unwrap_or_else(|| name.clone());
            println!("  {name} [managed] - {title}");
        }
        any = true;
    }

    if !any {
        eprintln!(
            "No skills found. Use `csa skill install <repo>` or `csa skill add <name>` to add skills."
        );
    }

    Ok(())
}

/// Handle `csa skill add <name>`.
///
/// Creates `<name>/SKILL.md` (and optional `.skill.toml`) in the managed skill
/// repo and commits the new files. Rollback on pre-commit failure: removes the
/// directory and resets the git index.
pub(crate) fn handle_skill_add(name: String) -> Result<()> {
    validate_skill_name(&name)?;

    let mgr = SkillRepoManager::new()?;
    mgr.ensure_init()?;

    mgr.with_write_lock(|| {
        let skill_dir = mgr.root().join(&name);

        if skill_dir.exists() {
            anyhow::bail!("Skill '{name}' already exists at {}", skill_dir.display());
        }

        fs::create_dir_all(&skill_dir)
            .with_context(|| format!("create skill dir {}", skill_dir.display()))?;

        let skill_md_path = skill_dir.join("SKILL.md");
        let template = skill_md_template(&name);
        fs::write(&skill_md_path, &template)
            .with_context(|| format!("write {}", skill_md_path.display()))?;

        let rel_skill_md = format!("{name}/SKILL.md");
        match git_commit_paths(mgr.root(), &[&rel_skill_md], &format!("add skill: {name}")) {
            Ok(_) => {
                eprintln!("Skill '{name}' created at {}", skill_dir.display());
                Ok(())
            }
            Err(e) => {
                // Rollback: remove files and reset git index.
                let _ = fs::remove_dir_all(&skill_dir);
                let _ = Command::new("git")
                    .args(["reset", "HEAD", "--", &format!("{name}/")])
                    .current_dir(mgr.root())
                    .output();
                Err(e).with_context(|| format!("failed to commit new skill '{name}'; rolled back"))
            }
        }
    })
}

/// Handle `csa skill edit <name>`.
///
/// Two-phase lock: opens `$EDITOR` outside the lock so concurrent reads are
/// not blocked during editing. Acquires the write lock only after the editor
/// exits, detects changes, and commits if the content was modified.
pub(crate) fn handle_skill_edit(name: String) -> Result<()> {
    validate_skill_name(&name)?;

    let mgr = SkillRepoManager::new()?;
    let skill_dir = mgr.root().join(&name);
    let skill_md_path = skill_dir.join("SKILL.md");

    if !skill_md_path.exists() {
        anyhow::bail!(
            "Skill '{name}' not found in managed repo at {}",
            skill_md_path.display()
        );
    }

    // Read content before editing for change detection.
    let before = fs::read_to_string(&skill_md_path)
        .with_context(|| format!("read {}", skill_md_path.display()))?;

    // Phase 1: open editor WITHOUT holding the write lock.
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = Command::new(&editor)
        .arg(&skill_md_path)
        .status()
        .with_context(|| format!("failed to launch editor '{editor}'"))?;

    if !status.success() {
        anyhow::bail!("Editor exited with non-zero status: {}", status);
    }

    // Phase 2: acquire write lock, check for changes, commit.
    mgr.with_write_lock(|| {
        let after = fs::read_to_string(&skill_md_path)
            .with_context(|| format!("read {}", skill_md_path.display()))?;

        if after == before {
            eprintln!("No changes detected in '{name}'.");
            return Ok(());
        }

        let rel_skill_md = format!("{name}/SKILL.md");
        git_commit_paths(mgr.root(), &[&rel_skill_md], &format!("edit skill: {name}"))?;
        eprintln!("Skill '{name}' saved.");
        Ok(())
    })
}

/// Handle `csa skill scan`.
///
/// Detects dirty state in the managed skill repo and commits all changes.
/// No-op when the repo is clean.
pub(crate) fn handle_skill_scan() -> Result<()> {
    let mgr = SkillRepoManager::new()?;
    mgr.ensure_init()?;

    mgr.with_write_lock(|| {
        let committed = git_commit_all(mgr.root(), "scan: commit untracked skill changes")?;
        if committed {
            eprintln!("Skill repo: changes committed.");
        } else {
            eprintln!("Skill repo: nothing to commit.");
        }
        Ok(())
    })
}

/// Handle `csa skill backup`.
///
/// Pushes the managed skill repo to a private GitHub backup remote.
/// Creates the remote repo on first run via `gh repo create`.
/// Uses default `gh` auth (not GH_CONFIG_DIR=~/.config/gh-aider).
pub(crate) fn handle_skill_backup() -> Result<()> {
    let repo_root = skill_repo_root()?;
    if !repo_root.exists() {
        anyhow::bail!("Managed skill repo not initialised. Run `csa skill add` first.");
    }

    // Determine GitHub username from `gh api user`.
    let user_out = Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .output()
        .context("run gh api user (is gh CLI installed?)")?;
    if !user_out.status.success() {
        anyhow::bail!(
            "gh api user failed: {}",
            String::from_utf8_lossy(&user_out.stderr)
        );
    }
    let username = String::from_utf8_lossy(&user_out.stdout).trim().to_string();
    if username.is_empty() {
        anyhow::bail!("Could not determine GitHub username from gh CLI");
    }

    let hostname = gethostname();
    let remote_name = "backup";
    let repo_name = format!("csa-skills-{hostname}");
    let remote_url = format!("https://github.com/{username}/{repo_name}");

    // Check whether the remote already exists.
    let remote_check = Command::new("git")
        .args(["remote", "get-url", remote_name])
        .current_dir(&repo_root)
        .output()
        .context("git remote get-url")?;

    if !remote_check.status.success() {
        // Create GitHub repo if it does not exist yet.
        eprintln!("Creating private GitHub repo: {username}/{repo_name}");
        let create_out = Command::new("gh")
            .args([
                "repo",
                "create",
                &format!("{username}/{repo_name}"),
                "--private",
                "--description",
                "CSA-managed skill backup",
            ])
            .output()
            .context("gh repo create")?;

        if !create_out.status.success() {
            let stderr = String::from_utf8_lossy(&create_out.stderr);
            // Tolerate "already exists" — just add the remote.
            if !stderr.contains("already exists") {
                anyhow::bail!("gh repo create failed: {stderr}");
            }
        }

        // Add remote.
        let add_out = Command::new("git")
            .args(["remote", "add", remote_name, &remote_url])
            .current_dir(&repo_root)
            .output()
            .context("git remote add")?;
        if !add_out.status.success() {
            anyhow::bail!(
                "git remote add failed: {}",
                String::from_utf8_lossy(&add_out.stderr)
            );
        }
    }

    // Determine current branch name.
    let branch_out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&repo_root)
        .output()
        .context("git rev-parse HEAD")?;
    let branch = if branch_out.status.success() {
        String::from_utf8_lossy(&branch_out.stdout)
            .trim()
            .to_string()
    } else {
        "main".to_string()
    };

    // Push to backup remote. No --no-verify needed: skill repo is isolated (no project hooks).
    eprintln!("Pushing to {remote_name}/{branch} ({remote_url})");
    let push_out = Command::new("git")
        .args(["push", remote_name, &branch])
        .current_dir(&repo_root)
        .output()
        .context("git push")?;
    if !push_out.status.success() {
        anyhow::bail!(
            "git push failed: {}",
            String::from_utf8_lossy(&push_out.stderr)
        );
    }

    eprintln!("Backup complete: {remote_url}");
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Collect all directories to search for active skills.
fn collect_active_skill_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // Project-local
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join(".claude").join("skills"));
        dirs.push(cwd.join(".csa").join("skills"));
    }

    // Global ~/.claude/skills/
    if let Some(base) = directories::BaseDirs::new() {
        dirs.push(base.home_dir().join(".claude").join("skills"));
    }

    dirs
}

/// Return the current hostname (best-effort, falls back to "host").
fn gethostname() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "host".to_string())
}

/// Default SKILL.md template for a new managed skill.
fn skill_md_template(name: &str) -> String {
    format!(
        "# {name}\n\n<!-- Describe what this skill does and when to invoke it. -->\n\n## Usage\n\nProvide task description as the prompt to `csa skill run {name}`.\n"
    )
}

/// Parse GitHub source to extract user/repo
fn parse_github_source(source: &str) -> Result<String> {
    if let Some(rest) = source.strip_prefix("https://github.com/") {
        // Extract user/repo from URL
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() >= 2 {
            return Ok(format!("{}/{}", parts[0], parts[1]));
        }
    }

    // Check if it matches user/repo pattern
    let parts: Vec<&str> = source.split('/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        return Ok(source.to_string());
    }

    anyhow::bail!("Invalid GitHub source format. Use 'user/repo' or 'https://github.com/user/repo'")
}

/// Clone repository using git
fn clone_repository(repo: &str, dest: &Path) -> Result<()> {
    let url = format!("https://github.com/{repo}");
    eprintln!("Cloning from: {url}");

    let output = Command::new("git")
        .args(["clone", "--depth", "1", &url, "."])
        .current_dir(dest)
        .output()
        .context("Failed to execute git clone. Is git installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Git clone failed: {stderr}");
    }

    info!("Successfully cloned repository");
    Ok(())
}

/// Find skills directory in cloned repo
fn find_skills_directory(repo_path: &Path) -> Result<PathBuf> {
    // Check .claude/skills/ first
    let claude_skills = repo_path.join(".claude").join("skills");
    if claude_skills.exists() && claude_skills.is_dir() {
        return Ok(claude_skills);
    }

    // Check skills/ directory
    let skills = repo_path.join("skills");
    if skills.exists() && skills.is_dir() {
        return Ok(skills);
    }

    anyhow::bail!(
        "No skills directory found. Expected '.claude/skills/' or 'skills/' in repository."
    )
}

/// Determine target directory based on target argument
fn determine_target_directory(target: Option<&str>) -> Result<PathBuf> {
    let target_tool = target.unwrap_or("claude-code");

    match target_tool {
        "claude-code" => get_claude_skills_dir(),
        "codex" | "opencode" => {
            // For now, these tools don't have a standard skills directory
            // We could add support later
            anyhow::bail!(
                "Skills for '{target_tool}' are not yet supported. Only 'claude-code' is supported."
            )
        }
        _ => anyhow::bail!("Unknown target tool: '{target_tool}'. Supported: claude-code"),
    }
}

/// Get Claude Code skills directory (~/.claude/skills/)
fn get_claude_skills_dir() -> Result<PathBuf> {
    let home = directories::BaseDirs::new()
        .context("Could not determine home directory")?
        .home_dir()
        .to_path_buf();
    Ok(home.join(".claude").join("skills"))
}

/// Copy skills from source to target directory
fn copy_skills(source_dir: &Path, target_dir: &Path) -> Result<Vec<String>> {
    let mut installed = Vec::new();

    let entries = fs::read_dir(source_dir).context("Failed to read skills directory")?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let skill_name = entry.file_name().to_string_lossy().to_string();
        let dest_path = target_dir.join(&skill_name);

        // Check if skill already exists
        if dest_path.exists() {
            eprintln!("Skipping '{skill_name}' (already exists)");
            continue;
        }

        // Copy directory recursively
        copy_dir_recursive(&path, &dest_path)
            .with_context(|| format!("Failed to copy skill '{skill_name}'"))?;

        installed.push(skill_name);
    }

    Ok(installed)
}

/// Recursively copy a directory
fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            fs::copy(&src_path, &dest_path)?;
        }
    }

    Ok(())
}

/// Read skill title from SKILL.md
fn read_skill_title(skill_dir: &Path) -> Option<String> {
    let skill_md = skill_dir.join("SKILL.md");
    if !skill_md.exists() {
        return None;
    }

    let content = fs::read_to_string(skill_md).ok()?;

    // Look for first # heading
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(title) = trimmed.strip_prefix('#') {
            return Some(title.trim().to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // --- parse_github_source tests ---

    #[test]
    fn parse_github_source_https_url() {
        let result = parse_github_source("https://github.com/user/repo").unwrap();
        assert_eq!(result, "user/repo");
    }

    #[test]
    fn parse_github_source_https_url_with_trailing_path() {
        let result = parse_github_source("https://github.com/user/repo/tree/main/skills").unwrap();
        assert_eq!(result, "user/repo");
    }

    #[test]
    fn parse_github_source_shorthand() {
        let result = parse_github_source("user/repo").unwrap();
        assert_eq!(result, "user/repo");
    }

    #[test]
    fn parse_github_source_invalid_single_word_errors() {
        assert!(parse_github_source("just-a-name").is_err());
    }

    #[test]
    fn parse_github_source_empty_parts_error() {
        assert!(parse_github_source("/repo").is_err());
        assert!(parse_github_source("user/").is_err());
    }

    #[test]
    fn parse_github_source_too_many_slashes_non_url() {
        // "a/b/c" has 3 parts, only 2 allowed for shorthand
        assert!(parse_github_source("a/b/c").is_err());
    }

    #[test]
    fn parse_github_source_empty_string_errors() {
        assert!(parse_github_source("").is_err());
    }

    // --- find_skills_directory tests ---

    #[test]
    fn find_skills_directory_claude_skills_preferred() {
        let tmp = tempdir().unwrap();
        let claude_skills = tmp.path().join(".claude").join("skills");
        fs::create_dir_all(&claude_skills).unwrap();
        // Also create skills/ to verify .claude/skills/ takes priority
        fs::create_dir_all(tmp.path().join("skills")).unwrap();

        let result = find_skills_directory(tmp.path()).unwrap();
        assert_eq!(result, claude_skills);
    }

    #[test]
    fn find_skills_directory_fallback_to_skills() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("skills")).unwrap();

        let result = find_skills_directory(tmp.path()).unwrap();
        assert_eq!(result, tmp.path().join("skills"));
    }

    #[test]
    fn find_skills_directory_none_found_errors() {
        let tmp = tempdir().unwrap();
        let result = find_skills_directory(tmp.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No skills directory"), "{}", err_msg);
    }

    // --- determine_target_directory tests ---

    #[test]
    fn determine_target_directory_default_claude_code() {
        let result = determine_target_directory(None);
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(
            path.ends_with(".claude/skills"),
            "expected .claude/skills suffix, got: {path:?}"
        );
    }

    #[test]
    fn determine_target_directory_explicit_claude_code() {
        let result = determine_target_directory(Some("claude-code"));
        assert!(result.is_ok());
    }

    #[test]
    fn determine_target_directory_unsupported_codex_errors() {
        let result = determine_target_directory(Some("codex"));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not yet supported")
        );
    }

    #[test]
    fn determine_target_directory_unknown_tool_errors() {
        let result = determine_target_directory(Some("vscode"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown target"));
    }

    // --- read_skill_title tests ---

    #[test]
    fn read_skill_title_with_heading() {
        let tmp = tempdir().unwrap();
        fs::write(
            tmp.path().join("SKILL.md"),
            "# My Cool Skill\n\nDescription",
        )
        .unwrap();
        let result = read_skill_title(tmp.path());
        assert_eq!(result, Some("My Cool Skill".to_string()));
    }

    #[test]
    fn read_skill_title_with_h2_heading() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("SKILL.md"), "## Sub Heading\n\nContent").unwrap();
        let result = read_skill_title(tmp.path());
        assert_eq!(result, Some("# Sub Heading".to_string()));
    }

    #[test]
    fn read_skill_title_no_heading_returns_none() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("SKILL.md"), "No headings here\nJust text").unwrap();
        let result = read_skill_title(tmp.path());
        assert!(result.is_none());
    }

    #[test]
    fn read_skill_title_no_file_returns_none() {
        let tmp = tempdir().unwrap();
        let result = read_skill_title(tmp.path());
        assert!(result.is_none());
    }
}
