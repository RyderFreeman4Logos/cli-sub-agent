use std::path::Path;

pub(super) const MEMORY_SOFT_LIMIT_NO_WORK_OUTCOME: &str = "no_tracked_repo_side_effects";
pub(super) const MEMORY_SOFT_LIMIT_DIRTY_OUTCOME: &str = "dirty_or_staged_changes";
pub(super) const MEMORY_SOFT_LIMIT_CLEAN_COMMITTED_OUTCOME: &str = "clean_committed_work";
pub(super) const MEMORY_SOFT_LIMIT_NO_WORK_ACTION: &str =
    "rerun_with_more_memory_or_reduce_parallelism";
pub(super) const MEMORY_SOFT_LIMIT_DIRTY_ACTION: &str =
    "inspect_git_status_preserve_changes_then_rerun_with_memory_headroom";
pub(super) const MEMORY_SOFT_LIMIT_REQUIRE_COMMIT_DIRTY_ACTION: &str =
    "inspect_git_status_preserve_staged_unstaged_then_retry_lightweight_commit_recovery";
pub(super) const MEMORY_SOFT_LIMIT_CLEAN_COMMITTED_ACTION: &str =
    "inspect_head_commit_then_continue";
pub(super) const MEMORY_SOFT_LIMIT_COMMIT_ONLY_RETRY_PROFILE: &str =
    "lightweight_commit_only_recovery";

pub(super) fn build_recovery_diagnostic(
    project_root: &Path,
    result: &csa_session::SessionResult,
    changes: Option<&csa_session::UncommittedChanges>,
    changed_paths: Option<&[String]>,
    commit_created: Option<bool>,
    require_commit: bool,
) -> Option<csa_session::MemorySoftLimitRecoveryDiagnostic> {
    if result.kill_hint.as_deref()
        != Some(csa_resource::memory_monitor::MEMORY_SOFT_LIMIT_KILL_HINT)
    {
        return None;
    }

    if let Some(changes) = changes {
        let (git_status_short, git_status_short_truncated) =
            bounded_git_status_short(project_root, changed_paths);
        let changed_paths = changes
            .files
            .iter()
            .map(|path| super::sanitize_diagnostic_path(path))
            .collect();
        let suggested_recovery_action = if require_commit {
            MEMORY_SOFT_LIMIT_REQUIRE_COMMIT_DIRTY_ACTION
        } else {
            MEMORY_SOFT_LIMIT_DIRTY_ACTION
        };
        return Some(csa_session::MemorySoftLimitRecoveryDiagnostic {
            outcome: MEMORY_SOFT_LIMIT_DIRTY_OUTCOME.to_string(),
            commit_created: commit_created.unwrap_or(false),
            dirty_worktree: true,
            changed_paths,
            changed_paths_truncated: changes.truncated,
            git_status_short,
            git_status_short_truncated,
            head_oid: None,
            head_summary: None,
            suggested_recovery_action: suggested_recovery_action.to_string(),
            retry_profile: require_commit
                .then_some(MEMORY_SOFT_LIMIT_COMMIT_ONLY_RETRY_PROFILE.to_string()),
        });
    }

    if commit_created.unwrap_or(false) {
        let (head_oid, head_summary) = current_head_commit_summary(project_root);
        let (changed_paths, changed_paths_truncated) =
            bounded_sanitized_paths(changed_paths.unwrap_or_default());
        return Some(csa_session::MemorySoftLimitRecoveryDiagnostic {
            outcome: MEMORY_SOFT_LIMIT_CLEAN_COMMITTED_OUTCOME.to_string(),
            commit_created: true,
            dirty_worktree: false,
            changed_paths,
            changed_paths_truncated,
            git_status_short: Vec::new(),
            git_status_short_truncated: 0,
            head_oid,
            head_summary,
            suggested_recovery_action: MEMORY_SOFT_LIMIT_CLEAN_COMMITTED_ACTION.to_string(),
            retry_profile: None,
        });
    }

    if changed_paths.is_some_and(|paths| paths.is_empty()) {
        return Some(csa_session::MemorySoftLimitRecoveryDiagnostic {
            outcome: MEMORY_SOFT_LIMIT_NO_WORK_OUTCOME.to_string(),
            commit_created: false,
            dirty_worktree: false,
            changed_paths: Vec::new(),
            changed_paths_truncated: 0,
            git_status_short: Vec::new(),
            git_status_short_truncated: 0,
            head_oid: None,
            head_summary: None,
            suggested_recovery_action: MEMORY_SOFT_LIMIT_NO_WORK_ACTION.to_string(),
            retry_profile: None,
        });
    }

    None
}

fn bounded_git_status_short(
    project_root: &Path,
    changed_paths: Option<&[String]>,
) -> (Vec<String>, usize) {
    let path_filter = changed_paths.map(|paths| {
        paths
            .iter()
            .filter(|path| !path.is_empty())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
    });
    let porcelain = super::run_git_capture(
        project_root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--no-renames",
            "-z",
        ],
    )
    .unwrap_or_default();
    let mut entries = Vec::new();
    for entry in super::porcelain_entries(&porcelain) {
        let Some(path) = super::parse_porcelain_path(entry) else {
            continue;
        };
        if let Some(filter) = path_filter.as_ref()
            && !filter.contains(&path)
        {
            continue;
        }
        let Some(status) = status_code(entry) else {
            continue;
        };
        entries.push(format!(
            "{status} {}",
            super::sanitize_diagnostic_path(&path)
        ));
    }
    let total = entries.len();
    entries.truncate(super::MAX_UNCOMMITTED_FILES);
    let truncated = total.saturating_sub(entries.len());
    (entries, truncated)
}

fn status_code(entry: &str) -> Option<String> {
    let mut chars = entry.chars();
    let first = chars.next()?;
    let second = chars.next()?;
    if first.is_control() || second.is_control() {
        return None;
    }
    Some(format!("{first}{second}"))
}

fn bounded_sanitized_paths(paths: &[String]) -> (Vec<String>, usize) {
    let total = paths.len();
    let paths = paths
        .iter()
        .take(super::MAX_UNCOMMITTED_FILES)
        .map(|path| super::sanitize_diagnostic_path(path))
        .collect::<Vec<_>>();
    let truncated = total.saturating_sub(paths.len());
    (paths, truncated)
}

fn current_head_commit_summary(project_root: &Path) -> (Option<String>, Option<String>) {
    let head_oid = super::run_git_capture(project_root, &["rev-parse", "--verify", "HEAD"])
        .map(|value| bounded_one_line(&value, 80))
        .filter(|value| !value.is_empty());
    let head_summary = super::run_git_capture(project_root, &["log", "-1", "--format=%s"])
        .map(|value| bounded_one_line(&value, 160))
        .filter(|value| !value.is_empty());
    (head_oid, head_summary)
}

fn bounded_one_line(value: &str, max_chars: usize) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(max_chars)
        .collect()
}
