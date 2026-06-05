use std::path::Path;
use std::process::Command;

pub fn detect_git_head(project_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(project_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let head = String::from_utf8(output.stdout).ok()?;
    let trimmed = head.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(super) fn detect_git_status_porcelain(project_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(project_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout).ok()
}

/// Detect a VCS change identifier for session-change binding using the active backend.
pub(super) fn detect_change_id(project_path: &Path) -> Option<String> {
    let backend = crate::vcs_backends::create_vcs_backend(project_path);
    backend.head_id(project_path).ok().flatten()
}

pub(super) fn detect_current_branch(project_path: &Path) -> Option<String> {
    let backend = crate::vcs_backends::create_vcs_backend(project_path);
    backend.current_branch(project_path).ok().flatten()
}
