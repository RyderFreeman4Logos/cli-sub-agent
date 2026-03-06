//! Git workspace snapshot and mutation guard helpers for `csa run`.
//!
//! Extracted from `run_cmd.rs` to keep module sizes manageable.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct GitWorkspaceSnapshot {
    pub(crate) head: Option<String>,
    pub(crate) status: String,
    pub(crate) tracked_worktree_fingerprint: Option<u64>,
    pub(crate) tracked_index_fingerprint: Option<u64>,
    pub(crate) untracked_fingerprint: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PostRunCommitGuard {
    pub(crate) workspace_mutated: bool,
    pub(crate) head_changed: bool,
    pub(crate) changed_paths: Vec<String>,
}

pub(crate) fn capture_git_workspace_snapshot(
    project_root: &Path,
    deep_fingerprint: bool,
) -> Option<GitWorkspaceSnapshot> {
    if !is_git_worktree(project_root) {
        return None;
    }

    let head = run_git_capture(project_root, &["rev-parse", "--verify", "HEAD"])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let status = run_git_capture(
        project_root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--no-renames",
            "-z",
        ],
    )?;
    let tracked_worktree_fingerprint = if deep_fingerprint {
        Some(capture_tracked_worktree_fingerprint(project_root, &status)?)
    } else {
        Some(capture_tracked_worktree_shallow_fingerprint(
            project_root,
            &status,
        ))
    };
    let tracked_index_fingerprint = Some(capture_tracked_index_fingerprint(project_root, &status)?);
    let untracked_fingerprint = Some(capture_untracked_fingerprint(
        project_root,
        deep_fingerprint,
    )?);

    Some(GitWorkspaceSnapshot {
        head,
        status,
        tracked_worktree_fingerprint,
        tracked_index_fingerprint,
        untracked_fingerprint,
    })
}

pub(crate) fn is_git_worktree(project_root: &Path) -> bool {
    run_git_capture(project_root, &["rev-parse", "--is-inside-work-tree"])
        .is_some_and(|value| value.trim() == "true")
}

fn run_git_capture(project_root: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_git_capture_with_paths(
    project_root: &Path,
    fixed_args: &[&str],
    paths: &[String],
) -> Option<String> {
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(project_root).args(fixed_args);
    for path in paths {
        command.arg(path);
    }

    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn hash_text(input: &str) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut hasher);
    hasher.finish()
}

fn capture_untracked_fingerprint(project_root: &Path, deep_content_hash: bool) -> Option<u64> {
    use std::hash::{Hash, Hasher};

    let raw_entries = run_git_capture(
        project_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;

    let paths: Vec<String> = raw_entries
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in &paths {
        path.hash(&mut hasher);
    }

    if deep_content_hash {
        let mut hashable_paths = Vec::new();
        for path in &paths {
            let full_path = project_root.join(path);
            match std::fs::symlink_metadata(&full_path) {
                Ok(metadata) if !metadata.is_dir() => hashable_paths.push(path.clone()),
                Ok(metadata) => {
                    "dir".hash(&mut hasher);
                    metadata.len().hash(&mut hasher);
                    if let Ok(modified) = metadata.modified()
                        && let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH)
                    {
                        since_epoch.as_secs().hash(&mut hasher);
                        since_epoch.subsec_nanos().hash(&mut hasher);
                    }
                }
                Err(_) => {
                    "missing".hash(&mut hasher);
                }
            }
        }

        if !hashable_paths.is_empty() {
            let content_hashes =
                run_git_capture_with_paths(project_root, &["hash-object", "--"], &hashable_paths)?;
            content_hashes.hash(&mut hasher);
        }

        return Some(hasher.finish());
    }

    for path in &paths {
        let full_path = project_root.join(path);
        if let Ok(metadata) = std::fs::metadata(&full_path) {
            metadata.len().hash(&mut hasher);
            if let Ok(modified) = metadata.modified()
                && let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                since_epoch.as_secs().hash(&mut hasher);
                since_epoch.subsec_nanos().hash(&mut hasher);
            }
        }
    }

    Some(hasher.finish())
}

fn capture_tracked_worktree_fingerprint(project_root: &Path, status: &str) -> Option<u64> {
    use std::hash::{Hash, Hasher};

    let paths = tracked_paths_from_status(status, |x, y| x != '?' && y != ' ');
    if paths.is_empty() {
        return Some(0);
    }

    let mut hashable_paths = Vec::new();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in &paths {
        path.hash(&mut hasher);
        let full_path = project_root.join(path);
        match std::fs::symlink_metadata(&full_path) {
            Ok(metadata) if !metadata.is_dir() => hashable_paths.push(path.clone()),
            Ok(metadata) => {
                "dir".hash(&mut hasher);
                metadata.len().hash(&mut hasher);
                if let Ok(modified) = metadata.modified()
                    && let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH)
                {
                    since_epoch.as_secs().hash(&mut hasher);
                    since_epoch.subsec_nanos().hash(&mut hasher);
                }
            }
            Err(_) => {
                "missing".hash(&mut hasher);
            }
        }
    }

    if !hashable_paths.is_empty() {
        let content_hashes =
            run_git_capture_with_paths(project_root, &["hash-object", "--"], &hashable_paths)?;
        content_hashes.hash(&mut hasher);
    }

    Some(hasher.finish())
}

fn capture_tracked_worktree_shallow_fingerprint(project_root: &Path, status: &str) -> u64 {
    let paths = tracked_paths_from_status(status, |x, y| x != '?' && y != ' ');
    hash_paths_and_metadata(project_root, &paths)
}

fn capture_tracked_index_fingerprint(project_root: &Path, status: &str) -> Option<u64> {
    let paths = tracked_paths_from_status(status, |x, _| x != ' ' && x != '?');
    if paths.is_empty() {
        return Some(0);
    }

    run_git_capture_with_paths(project_root, &["ls-files", "--stage", "--"], &paths)
        .map(|output| hash_text(&output))
}

pub(crate) fn tracked_paths_from_status(
    status: &str,
    include: impl Fn(char, char) -> bool,
) -> Vec<String> {
    collect_status_entries(status)
        .into_iter()
        .filter_map(|entry| {
            let (x, y, path) = parse_status_entry(entry)?;
            if !include(x, y) {
                return None;
            }
            Some(path.to_string())
        })
        .collect()
}

fn collect_status_entries(status: &str) -> Vec<&str> {
    if status.contains('\0') {
        status
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .collect()
    } else {
        status.lines().filter(|entry| !entry.is_empty()).collect()
    }
}

fn parse_status_entry(entry: &str) -> Option<(char, char, &str)> {
    let mut chars = entry.chars();
    let x = chars.next()?;
    let y = chars.next()?;
    if chars.next()? != ' ' {
        return None;
    }
    let path = entry.get(3..)?;
    if path.is_empty() {
        return None;
    }
    Some((x, y, path))
}

fn hash_paths_and_metadata(project_root: &Path, paths: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};

    if paths.is_empty() {
        return 0;
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in paths {
        path.hash(&mut hasher);

        let full_path = project_root.join(path);
        if let Ok(metadata) = std::fs::metadata(&full_path) {
            metadata.len().hash(&mut hasher);
            if let Ok(modified) = metadata.modified()
                && let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                since_epoch.as_secs().hash(&mut hasher);
                since_epoch.subsec_nanos().hash(&mut hasher);
            }
        }
    }

    hasher.finish()
}

pub(crate) fn evaluate_post_run_commit_guard(
    before: Option<&GitWorkspaceSnapshot>,
    after: Option<&GitWorkspaceSnapshot>,
) -> Option<PostRunCommitGuard> {
    let after = after?;
    let before = before?;
    if after.status.trim().is_empty() {
        return None;
    }

    let tracked_fingerprint_changed = before.status != after.status
        || before.tracked_worktree_fingerprint != after.tracked_worktree_fingerprint
        || before.tracked_index_fingerprint != after.tracked_index_fingerprint;
    let untracked_changed = before.untracked_fingerprint != after.untracked_fingerprint;
    let workspace_mutated = tracked_fingerprint_changed || untracked_changed;
    if !workspace_mutated {
        return None;
    }

    Some(PostRunCommitGuard {
        workspace_mutated,
        head_changed: before.head != after.head,
        changed_paths: changed_paths_from_status(&after.status, 8),
    })
}

pub(crate) fn changed_paths_from_status(status: &str, limit: usize) -> Vec<String> {
    collect_status_entries(status)
        .into_iter()
        .filter_map(|entry| parse_status_entry(entry).map(|(_, _, path)| path.to_string()))
        .take(limit)
        .collect()
}
