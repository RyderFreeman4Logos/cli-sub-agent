use super::session_exec_pre_exec::persist_pipeline_pre_exec_failure;
use crate::session_guard::SessionCleanupGuard;
use anyhow::{Context, Result};
use csa_lock::WorktreeWriteLock;
use csa_session::MetaSessionState;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn acquire_or_persist_failure(
    project_root: &Path,
    session: &mut MetaSessionState,
    tool_name: &str,
    task_type: Option<&str>,
    readonly_project_root: bool,
    cleanup_guard: &mut Option<SessionCleanupGuard>,
) -> Result<Option<WorktreeWriteLock>> {
    acquire_if_needed(project_root, session, task_type, readonly_project_root).map_err(|err| {
        persist_pipeline_pre_exec_failure(
            project_root,
            session,
            tool_name,
            err,
            cleanup_guard,
            None,
        )
    })
}

pub(super) fn acquire_if_needed(
    project_root: &Path,
    session: &MetaSessionState,
    task_type: Option<&str>,
    readonly_project_root: bool,
) -> Result<Option<WorktreeWriteLock>> {
    if !is_write_session(task_type, readonly_project_root) {
        return Ok(None);
    }

    let worktree_root = resolve_worktree_lock_root(project_root)?;
    let ancestor_session_ids = collect_lineage_session_ids(project_root, session)?;
    csa_lock::acquire_worktree_write_lock(
        &worktree_root,
        &session.meta_session_id,
        &ancestor_session_ids,
    )
    .map(Some)
}

fn is_write_session(task_type: Option<&str>, readonly_project_root: bool) -> bool {
    matches!(task_type, Some("run")) && !readonly_project_root
}

fn resolve_worktree_lock_root(project_root: &Path) -> Result<PathBuf> {
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

fn collect_lineage_session_ids(
    project_root: &Path,
    session: &MetaSessionState,
) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    push_lineage(
        project_root,
        session.genealogy.parent_session_id.as_deref(),
        &mut ids,
        &mut seen,
    )?;
    push_lineage(
        project_root,
        session.genealogy.fork_source(),
        &mut ids,
        &mut seen,
    )?;
    Ok(ids)
}

fn push_lineage(
    project_root: &Path,
    first_session_id: Option<&str>,
    ids: &mut Vec<String>,
    seen: &mut HashSet<String>,
) -> Result<()> {
    let mut current = first_session_id.map(ToString::to_string);
    while let Some(session_id) = current {
        if !seen.insert(session_id.clone()) {
            break;
        }
        ids.push(session_id.clone());
        let state = csa_session::load_session(project_root, &session_id)
            .with_context(|| format!("failed to load ancestor session {session_id}"))?;
        current = state
            .genealogy
            .parent_session_id
            .clone()
            .or_else(|| state.genealogy.fork_of_session_id.clone());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_lock_predicate_only_matches_writable_run_sessions() {
        assert!(is_write_session(Some("run"), false));
        assert!(!is_write_session(Some("run"), true));
        assert!(!is_write_session(Some("reviewer_sub_session"), false));
        assert!(!is_write_session(Some("debate"), false));
        assert!(!is_write_session(None, false));
    }
}
