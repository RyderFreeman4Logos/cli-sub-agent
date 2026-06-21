use std::path::Path;

pub(super) const MEMORY_SOFT_LIMIT_NO_WORK_OUTCOME: &str = "no_tracked_repo_side_effects";
pub(super) const MEMORY_SOFT_LIMIT_DIRTY_OUTCOME: &str = "dirty_or_staged_changes";
pub(super) const MEMORY_SOFT_LIMIT_CLEAN_COMMITTED_OUTCOME: &str = "clean_committed_work";
pub(super) const MEMORY_SOFT_LIMIT_NO_WORK_ACTION: &str =
    "rerun_with_more_memory_or_reduce_parallelism";
pub(super) const MEMORY_SOFT_LIMIT_DIRTY_ACTION: &str =
    "inspect_changed_paths_then_salvage_or_revert";
pub(super) const MEMORY_SOFT_LIMIT_CLEAN_COMMITTED_ACTION: &str =
    "inspect_head_commit_then_continue";

pub(super) fn build_recovery_diagnostic(
    project_root: &Path,
    result: &csa_session::SessionResult,
    changes: Option<&csa_session::UncommittedChanges>,
    changed_paths: Option<&[String]>,
    commit_created: Option<bool>,
) -> Option<csa_session::MemorySoftLimitRecoveryDiagnostic> {
    if result.kill_hint.as_deref()
        != Some(csa_resource::memory_monitor::MEMORY_SOFT_LIMIT_KILL_HINT)
    {
        return None;
    }

    if let Some(changes) = changes {
        let changed_paths = changes
            .files
            .iter()
            .map(|path| super::sanitize_diagnostic_path(path))
            .collect();
        return Some(csa_session::MemorySoftLimitRecoveryDiagnostic {
            outcome: MEMORY_SOFT_LIMIT_DIRTY_OUTCOME.to_string(),
            commit_created: commit_created.unwrap_or(false),
            dirty_worktree: true,
            changed_paths,
            changed_paths_truncated: changes.truncated,
            head_oid: None,
            head_summary: None,
            suggested_recovery_action: MEMORY_SOFT_LIMIT_DIRTY_ACTION.to_string(),
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
            head_oid,
            head_summary,
            suggested_recovery_action: MEMORY_SOFT_LIMIT_CLEAN_COMMITTED_ACTION.to_string(),
        });
    }

    if changed_paths.is_some_and(|paths| paths.is_empty()) {
        return Some(csa_session::MemorySoftLimitRecoveryDiagnostic {
            outcome: MEMORY_SOFT_LIMIT_NO_WORK_OUTCOME.to_string(),
            commit_created: false,
            dirty_worktree: false,
            changed_paths: Vec::new(),
            changed_paths_truncated: 0,
            head_oid: None,
            head_summary: None,
            suggested_recovery_action: MEMORY_SOFT_LIMIT_NO_WORK_ACTION.to_string(),
        });
    }

    None
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
