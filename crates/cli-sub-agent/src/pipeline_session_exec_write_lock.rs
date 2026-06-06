use super::session_exec_pre_exec::persist_pipeline_pre_exec_failure;
use crate::session_guard::SessionCleanupGuard;
use anyhow::{Context, Result};
use csa_lock::WorktreeWriteLock;
use csa_session::{MetaSessionState, SessionPhase};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn acquire_or_persist_failure(
    project_root: &Path,
    session: &mut MetaSessionState,
    tool_name: &str,
    readonly_project_root: bool,
    cleanup_guard: &mut Option<SessionCleanupGuard>,
) -> Result<Option<WorktreeWriteLock>> {
    acquire_if_needed(project_root, session, readonly_project_root).map_err(|err| {
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
    readonly_project_root: bool,
) -> Result<Option<WorktreeWriteLock>> {
    if !session_mutates_worktree(readonly_project_root) {
        return Ok(None);
    }

    let worktree_root = resolve_worktree_lock_root(project_root)?;
    let ancestor_session_ids = collect_lineage_session_ids(project_root, session)?;
    csa_lock::acquire_worktree_write_lock(
        &worktree_root,
        &session.meta_session_id,
        &ancestor_session_ids,
        |holder_session_id| holder_session_is_active(project_root, holder_session_id),
    )
    .map(Some)
}

/// Whether this session can mutate the shared git worktree and therefore must
/// serialize against other writers via the per-worktree write lock (#1672).
///
/// The mutation signal is `readonly_project_root == false`: any session whose
/// project root is NOT mounted read-only can write to the shared `.git`/working
/// tree, regardless of its session-type classification. Keying on the session
/// type (e.g. only `csa run`) silently excluded `csa review --fix` and
/// `csa debate` write-modes — which set `readonly_project_root = false` yet are
/// classified as `reviewer_sub_session`/`debate` — letting them race a
/// concurrent writer and clobber a commit (#1828). Deriving the predicate from
/// `readonly_project_root` also covers future write-capable session kinds
/// automatically. Pure read-only review/debate (`readonly_project_root == true`)
/// acquires no lock and never contends.
fn session_mutates_worktree(readonly_project_root: bool) -> bool {
    !readonly_project_root
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

fn holder_session_is_active(project_root: &Path, session_id: &str) -> bool {
    match csa_session::load_session(project_root, session_id) {
        Ok(state) => state.phase == SessionPhase::Active,
        Err(err) => {
            tracing::debug!(
                session_id,
                error = %err,
                "treating unreadable lineage holder session as not live"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_mutation_predicate_keys_on_readonly_not_session_type() {
        // Any writable session mutates the shared worktree and must take the
        // lock, regardless of session-type classification (#1828). This covers
        // `csa run` write sessions, `csa review --fix`, and `csa debate`
        // write-modes alike — session-type coverage is asserted end-to-end in
        // `pipeline_tests_locking`.
        assert!(session_mutates_worktree(false));
        // A read-only sandbox cannot mutate the worktree → no lock, no
        // contention.
        assert!(!session_mutates_worktree(true));
    }

    #[test]
    fn acquire_if_needed_blocks_post_exec_holder_while_guard_is_live() {
        let temp = tempfile::tempdir().unwrap();
        let holder =
            csa_session::create_session_fresh(temp.path(), Some("done"), None, Some("codex"))
                .expect("create holder session");
        save_success_result(temp.path(), &holder.meta_session_id);
        let holder_lock = csa_lock::acquire_worktree_write_lock(
            temp.path(),
            &holder.meta_session_id,
            &[],
            |_| false,
        )
        .expect("holder lock");
        let lock_path = holder_lock.lock_path().to_path_buf();
        let candidate =
            csa_session::create_session_fresh(temp.path(), Some("next"), None, Some("codex"))
                .expect("create candidate session");

        let err = acquire_if_needed(temp.path(), &candidate, false)
            .expect_err("completed holder with live flock must block")
            .to_string();

        assert!(err.contains("concurrent write session blocked"));
        assert!(err.contains(&holder.meta_session_id));
        assert!(
            lock_path.exists(),
            "canonical lock path must not be moved aside"
        );
    }

    #[test]
    fn acquire_if_needed_keeps_active_holder_session_blocked() {
        let temp = tempfile::tempdir().unwrap();
        let holder =
            csa_session::create_session_fresh(temp.path(), Some("running"), None, Some("codex"))
                .expect("create holder session");
        let _holder_lock = csa_lock::acquire_worktree_write_lock(
            temp.path(),
            &holder.meta_session_id,
            &[],
            |_| false,
        )
        .expect("holder lock");
        let candidate =
            csa_session::create_session_fresh(temp.path(), Some("next"), None, Some("codex"))
                .expect("create candidate session");

        let err = acquire_if_needed(temp.path(), &candidate, false)
            .expect_err("active holder with no result must still block")
            .to_string();

        assert!(err.contains("concurrent write session blocked"));
        assert!(err.contains(&holder.meta_session_id));
    }

    fn save_success_result(project_root: &Path, session_id: &str) {
        csa_session::save_result(
            project_root,
            session_id,
            &csa_session::SessionResult {
                status: "success".to_string(),
                exit_code: 0,
                summary: "done".to_string(),
                tool: "codex".to_string(),
                original_tool: None,
                fallback_tool: None,
                fallback_reason: None,
                started_at: chrono::Utc::now(),
                completed_at: chrono::Utc::now(),
                events_count: 0,
                artifacts: Vec::new(),
                peak_memory_mb: None,
                fallback_chain: None,
                gate_timeout: false,
                warnings: Vec::new(),
                raw_process_exit_code: None,
                uncommitted_changes: None,
                post_exec_gate: None,
                manager_fields: Default::default(),
            },
        )
        .expect("save result");
    }
}
