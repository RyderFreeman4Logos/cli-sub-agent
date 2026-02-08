use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use tracing::info;

/// Handle skill install command
pub(crate) fn handle_skill_install(source: String, target: Option<String>) -> Result<()> {
    // 1. Parse source to get user/repo
    let repo = parse_github_source(&source)?;
    eprintln!("Installing skills from: {}", repo);

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
            eprintln!("  - {}", skill);
        }
    }

    Ok(())
}

/// Handle skill list command
pub(crate) fn handle_skill_list() -> Result<()> {
    let skills_dir = get_claude_skills_dir()?;

    if !skills_dir.exists() {
        eprintln!("No skills directory found at: {}", skills_dir.display());
        return Ok(());
    }

    let entries: Vec<_> = fs::read_dir(&skills_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();

    if entries.is_empty() {
        eprintln!("No skills installed.");
        return Ok(());
    }

    eprintln!("Installed skills:\n");
    for entry in entries {
        let skill_name = entry.file_name().to_string_lossy().to_string();
        let skill_path = entry.path();

        // Try to read SKILL.md to get title
        let title = read_skill_title(&skill_path).unwrap_or_else(|| skill_name.clone());

        println!("  {} - {}", skill_name, title);
    }

    Ok(())
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
    let url = format!("https://github.com/{}", repo);
    eprintln!("Cloning from: {}", url);

    let output = Command::new("git")
        .args(["clone", "--depth", "1", &url, "."])
        .current_dir(dest)
        .output()
        .context("Failed to execute git clone. Is git installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Git clone failed: {}", stderr);
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
                "Skills for '{}' are not yet supported. Only 'claude-code' is supported.",
                target_tool
            )
        }
        _ => anyhow::bail!(
            "Unknown target tool: '{}'. Supported: claude-code",
            target_tool
        ),
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
            eprintln!("Skipping '{}' (already exists)", skill_name);
            continue;
        }

        // Copy directory recursively
        copy_dir_recursive(&path, &dest_path)
            .with_context(|| format!("Failed to copy skill '{}'", skill_name))?;

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
