use std::path::Path;
use std::process::Command;

pub(super) fn snapshot_to_fingerprints(
    snap: &crate::run_cmd::GitWorkspaceSnapshot,
) -> crate::pipeline::changed_paths::SnapshotFingerprints {
    crate::pipeline::changed_paths::SnapshotFingerprints {
        tracked_worktree: snap.tracked_worktree_fingerprint,
        tracked_index: snap.tracked_index_fingerprint,
        untracked: snap.untracked_fingerprint,
    }
}

pub(super) fn capture_git_workspace_snapshot_if_needed(
    is_git: bool,
    project_root: &Path,
    deep_fingerprint: bool,
) -> Option<crate::run_cmd::GitWorkspaceSnapshot> {
    is_git
        .then(|| crate::run_cmd::capture_git_workspace_snapshot(project_root, deep_fingerprint))?
}

pub(super) fn capture_pre_execution_snapshot(
    project_root: &Path,
) -> Option<crate::pipeline_post_exec::PreExecutionSnapshot> {
    let head = detect_git_head(project_root)?;
    let porcelain = detect_git_status_porcelain(project_root);
    Some(crate::pipeline_post_exec::PreExecutionSnapshot { head, porcelain })
}

fn detect_git_head(project_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(project_root)
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

fn detect_git_status_porcelain(project_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(project_root)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout).ok()
}
