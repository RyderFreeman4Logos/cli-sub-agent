use std::path::Path;

/// Detect whether `project_root` resides inside a git worktree submodule.
///
/// A worktree submodule's `.git` is a file (not directory) containing a
/// `gitdir:` reference that traverses both `worktrees/` and `modules/`
/// path segments — the hallmark of the nested worktree-submodule layout.
pub(in crate::review_cmd) fn is_worktree_submodule(project_root: &Path) -> bool {
    let git_marker = project_root.join(".git");
    if !git_marker.is_file() {
        return false;
    }
    let Ok(marker) = std::fs::read_to_string(&git_marker) else {
        return false;
    };
    let Some(gitdir_raw) = marker.trim().strip_prefix("gitdir:") else {
        return false;
    };
    let gitdir = gitdir_raw.trim();
    gitdir.contains("/worktrees/") && gitdir.contains("/modules/")
}
