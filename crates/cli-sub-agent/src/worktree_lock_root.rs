use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn resolve_worktree_lock_root(project_root: &Path) -> Result<PathBuf> {
    if let Some(toplevel) = git_rev_parse_path(project_root, "--show-toplevel") {
        return canonicalize_lock_root(&toplevel, "git worktree toplevel");
    }

    if let Some(common_dir) = git_rev_parse_path(project_root, "--git-common-dir") {
        return canonicalize_lock_root(&common_dir, "git common dir");
    }

    canonicalize_lock_root(project_root, "project root")
}

fn canonicalize_lock_root(path: &Path, label: &str) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("failed to canonicalize {label} '{}'", path.display()))
}

fn git_rev_parse_path(project_root: &Path, arg: &str) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .arg("rev-parse")
        .arg(arg)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }

    let path = PathBuf::from(raw);
    if path.is_absolute() {
        Some(path)
    } else {
        Some(project_root.join(path))
    }
}
