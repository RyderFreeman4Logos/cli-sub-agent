use super::session_exec_pre_exec::persist_pipeline_pre_exec_failure;
use crate::session_guard::SessionCleanupGuard;
use anyhow::{Context, Result};
use csa_lock::HolderSessionLiveness;
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
    csa_lock::acquire_worktree_write_lock_with_liveness(
        &worktree_root,
        &session.meta_session_id,
        &ancestor_session_ids,
        |holder_session_id| holder_session_liveness(project_root, holder_session_id),
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

fn holder_session_liveness(project_root: &Path, session_id: &str) -> HolderSessionLiveness {
    match csa_session::load_session(project_root, session_id) {
        Ok(session) => classify_loaded_holder_session(&session),
        Err(project_err) => match csa_session::load_session_global_exact(session_id) {
            Ok(Some(session)) => classify_loaded_holder_session(&session),
            Ok(None) => HolderSessionLiveness::RegistryAbsent,
            Err(global_err) => {
                tracing::warn!(
                    session_id,
                    project_error = %project_err,
                    global_error = %global_err,
                    "could not determine worktree lock holder session liveness"
                );
                HolderSessionLiveness::Unknown
            }
        },
    }
}

fn classify_loaded_holder_session(session: &MetaSessionState) -> HolderSessionLiveness {
    let project_root = Path::new(&session.project_path);
    let session_dir = match csa_session::get_session_dir(project_root, &session.meta_session_id) {
        Ok(dir) => dir,
        Err(err) => {
            tracing::warn!(
                session_id = %session.meta_session_id,
                error = %err,
                "could not resolve worktree lock holder session dir"
            );
            return HolderSessionLiveness::Unknown;
        }
    };

    if csa_process::ToolLiveness::has_live_process(&session_dir)
        || csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir)
    {
        return HolderSessionLiveness::Live;
    }

    match csa_session::load_result(project_root, &session.meta_session_id) {
        Ok(Some(_)) => HolderSessionLiveness::Dead,
        Err(err) => {
            tracing::warn!(
                session_id = %session.meta_session_id,
                error = %err,
                "could not read worktree lock holder result"
            );
            HolderSessionLiveness::Unknown
        }
        Ok(None) if matches!(session.phase, SessionPhase::Active) => HolderSessionLiveness::Live,
        Ok(None) => HolderSessionLiveness::Dead,
    }
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
    use chrono::Utc;
    use std::fs;

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
    fn holder_session_liveness_treats_completed_active_session_as_dead() {
        let temp = tempfile::tempdir().unwrap();
        let session =
            csa_session::create_session_fresh(temp.path(), Some("done"), None, Some("codex"))
                .expect("create session");
        save_success_result(temp.path(), &session.meta_session_id);

        assert_eq!(
            holder_session_liveness(temp.path(), &session.meta_session_id),
            HolderSessionLiveness::Dead
        );
    }

    #[test]
    fn holder_session_liveness_keeps_active_session_without_result_live() {
        let temp = tempfile::tempdir().unwrap();
        let session =
            csa_session::create_session_fresh(temp.path(), Some("running"), None, Some("codex"))
                .expect("create session");

        assert_eq!(
            holder_session_liveness(temp.path(), &session.meta_session_id),
            HolderSessionLiveness::Live
        );
    }

    #[test]
    fn holder_session_liveness_keeps_missing_registry_entry_distinct_from_dead() {
        let temp = tempfile::tempdir().unwrap();
        let missing_session_id = csa_session::new_session_id();

        assert_eq!(
            holder_session_liveness(temp.path(), &missing_session_id),
            HolderSessionLiveness::RegistryAbsent
        );
    }

    #[test]
    fn acquire_if_needed_blocks_completed_holder_session_with_signalable_pid() {
        let temp = tempfile::tempdir().unwrap();
        let holder =
            csa_session::create_session_fresh(temp.path(), Some("done"), None, Some("codex"))
                .expect("create holder session");
        save_success_result(temp.path(), &holder.meta_session_id);
        let holder_lock =
            csa_lock::acquire_worktree_write_lock(temp.path(), &holder.meta_session_id, &[])
                .expect("holder lock");
        let lock_path = holder_lock.lock_path().to_path_buf();
        let candidate =
            csa_session::create_session_fresh(temp.path(), Some("next"), None, Some("codex"))
                .expect("create candidate session");
        assert_eq!(
            holder_session_liveness(temp.path(), &holder.meta_session_id),
            HolderSessionLiveness::Dead
        );

        let err = acquire_if_needed(temp.path(), &candidate, false)
            .expect_err("completed holder with signalable pid must block")
            .to_string();

        assert!(err.contains("concurrent write session blocked"));
        assert!(err.contains(&holder.meta_session_id));
        assert!(
            lock_path.exists(),
            "canonical lock path must not be moved aside"
        );
    }

    #[test]
    fn acquire_if_needed_reclaims_registry_absent_holder_only_when_pid_missing() {
        let temp = tempfile::tempdir().unwrap();
        let holder =
            csa_session::create_session_fresh(temp.path(), Some("gone"), None, Some("codex"))
                .expect("create holder session");
        let holder_session_id = holder.meta_session_id.clone();
        let holder_lock =
            csa_lock::acquire_worktree_write_lock(temp.path(), &holder_session_id, &[])
                .expect("holder lock");
        overwrite_worktree_lock_diagnostic(
            holder_lock.lock_path(),
            missing_pid(),
            &holder_session_id,
            temp.path(),
        );
        csa_session::delete_session(temp.path(), &holder_session_id).expect("delete holder");
        let candidate =
            csa_session::create_session_fresh(temp.path(), Some("next"), None, Some("codex"))
                .expect("create candidate session");
        assert_eq!(
            holder_session_liveness(temp.path(), &holder_session_id),
            HolderSessionLiveness::RegistryAbsent
        );

        let lock = acquire_if_needed(temp.path(), &candidate, false)
            .expect("missing registry entry with dead pid should be reclaimed")
            .expect("writer should acquire a lock");

        assert!(!lock.is_lineage_reentry());
        drop(holder_lock);
        drop(lock);
    }

    #[test]
    fn acquire_if_needed_keeps_active_holder_session_blocked() {
        let temp = tempfile::tempdir().unwrap();
        let holder =
            csa_session::create_session_fresh(temp.path(), Some("running"), None, Some("codex"))
                .expect("create holder session");
        let _holder_lock =
            csa_lock::acquire_worktree_write_lock(temp.path(), &holder.meta_session_id, &[])
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

    #[test]
    fn acquire_if_needed_blocks_registry_absent_holder_with_signalable_pid() {
        let temp = tempfile::tempdir().unwrap();
        let holder =
            csa_session::create_session_fresh(temp.path(), Some("missing"), None, Some("codex"))
                .expect("create holder session");
        let holder_session_id = holder.meta_session_id.clone();
        let holder_lock =
            csa_lock::acquire_worktree_write_lock(temp.path(), &holder_session_id, &[])
                .expect("holder lock");
        let lock_path = holder_lock.lock_path().to_path_buf();
        overwrite_worktree_lock_diagnostic(
            &lock_path,
            std::process::id(),
            &holder_session_id,
            temp.path(),
        );
        csa_session::delete_session(temp.path(), &holder_session_id).expect("delete holder");
        let candidate =
            csa_session::create_session_fresh(temp.path(), Some("next"), None, Some("codex"))
                .expect("create candidate session");
        assert_eq!(
            holder_session_liveness(temp.path(), &holder_session_id),
            HolderSessionLiveness::RegistryAbsent
        );

        let err = acquire_if_needed(temp.path(), &candidate, false)
            .expect_err("missing registry entry with signalable pid must block")
            .to_string();

        assert!(err.contains("concurrent write session blocked"));
        assert!(err.contains(&holder_session_id));
        assert!(
            lock_path.exists(),
            "canonical lock path must not be moved aside"
        );
    }

    #[test]
    fn acquire_if_needed_blocks_unreadable_holder_state_with_signalable_pid() {
        let temp = tempfile::tempdir().unwrap();
        let holder =
            csa_session::create_session_fresh(temp.path(), Some("corrupt"), None, Some("codex"))
                .expect("create holder session");
        let holder_session_id = holder.meta_session_id.clone();
        let holder_lock =
            csa_lock::acquire_worktree_write_lock(temp.path(), &holder_session_id, &[])
                .expect("holder lock");
        let lock_path = holder_lock.lock_path().to_path_buf();
        overwrite_worktree_lock_diagnostic(
            &lock_path,
            std::process::id(),
            &holder_session_id,
            temp.path(),
        );
        let holder_dir =
            csa_session::get_session_dir(temp.path(), &holder_session_id).expect("holder dir");
        fs::write(holder_dir.join("state.toml"), "not valid toml").expect("corrupt state");
        let candidate =
            csa_session::create_session_fresh(temp.path(), Some("next"), None, Some("codex"))
                .expect("create candidate session");
        assert_eq!(
            holder_session_liveness(temp.path(), &holder_session_id),
            HolderSessionLiveness::Unknown
        );

        let err = acquire_if_needed(temp.path(), &candidate, false)
            .expect_err("unreadable holder state must block")
            .to_string();

        assert!(err.contains("concurrent write session blocked"));
        assert!(err.contains(&holder_session_id));
        assert!(
            lock_path.exists(),
            "canonical lock path must not be moved aside"
        );
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

    fn overwrite_worktree_lock_diagnostic(
        lock_path: &Path,
        pid: u32,
        session_id: &str,
        worktree_root: &Path,
    ) {
        let diagnostic = serde_json::json!({
            "pid": pid,
            "tool_name": "worktree-write:exclusive",
            "acquired_at": Utc::now(),
            "reason": format!(
                "write session {session_id} holds worktree {}",
                worktree_root.display()
            ),
            "holder_session_id": session_id,
            "resource_path": worktree_root.display().to_string(),
        });
        fs::write(lock_path, serde_json::to_string(&diagnostic).unwrap())
            .expect("overwrite lock diagnostic");
    }

    fn missing_pid() -> u32 {
        [4_000_000, 8_000_000, 16_000_000, 1_000_000_000]
            .into_iter()
            .find(|pid| process_is_missing(*pid))
            .expect("test host should have at least one definitely missing pid")
    }

    fn process_is_missing(pid: u32) -> bool {
        // SAFETY: signal 0 checks process existence without sending a signal.
        let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
        ret != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}
