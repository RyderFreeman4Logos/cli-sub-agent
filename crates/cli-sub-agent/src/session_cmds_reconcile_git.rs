use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

pub(super) fn git_output(project_root: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .with_context(|| format!("Failed to run git {:?}", args))
}

pub(super) fn git_success(project_root: &Path, args: &[&str]) -> bool {
    git_output(project_root, args)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn git_quiet_stdout(project_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(project_root)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_branch_name(candidate: &str) -> Option<String> {
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(
        trimmed
            .strip_prefix("refs/remotes/origin/")
            .or_else(|| trimmed.strip_prefix("origin/"))
            .unwrap_or(trimmed)
            .to_string(),
    )
}

pub(super) fn resolve_fallback_base_branch(project_root: &Path) -> Option<String> {
    git_quiet_stdout(
        project_root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .and_then(|value| normalize_branch_name(&value))
    .or_else(|| {
        git_quiet_stdout(project_root, &["rev-parse", "--abbrev-ref", "@{upstream}"])
            .and_then(|value| normalize_branch_name(&value))
    })
    .or_else(|| {
        git_quiet_stdout(project_root, &["config", "init.defaultBranch"])
            .and_then(|value| normalize_branch_name(&value))
    })
    .or_else(|| {
        ["main", "master"].into_iter().find_map(|candidate| {
            git_success(
                project_root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{candidate}"),
                ],
            )
            .then(|| candidate.to_string())
        })
    })
}
