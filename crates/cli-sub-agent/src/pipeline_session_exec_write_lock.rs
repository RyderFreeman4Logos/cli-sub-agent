use super::session_exec_pre_exec::{
    PipelinePreExecFailureDetails, persist_pipeline_pre_exec_failure,
};
use crate::session_guard::SessionCleanupGuard;
use anyhow::{Context, Result};
use csa_lock::WorktreeWriteLock;
use csa_session::{MetaSessionState, SessionPhase};
use std::collections::HashSet;
use std::path::Path;

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
            PipelinePreExecFailureDetails::absent(),
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

    let worktree_root = crate::worktree_lock_root::resolve_worktree_lock_root(project_root)?;
    let ancestor_session_ids = collect_lineage_session_ids(project_root, session)?;
    csa_lock::acquire_worktree_write_lock(
        &worktree_root,
        &session.meta_session_id,
        &ancestor_session_ids,
        |holder_session_id| holder_session_is_active(project_root, holder_session_id),
        |holder_session_id| holder_session_is_terminal(project_root, holder_session_id),
        |holder_session_id| holder_session_result_is_stale(project_root, holder_session_id),
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

fn holder_session_is_terminal(project_root: &Path, session_id: &str) -> bool {
    match csa_session::load_session(project_root, session_id) {
        // Only Retired and ToolExhausted indicate the session lifecycle has
        // fully completed and the worktree lock guard has been dropped.
        // Available means the session was compacted but may still hold the
        // lock during post-exec finalization.
        Ok(state) => matches!(
            state.phase,
            SessionPhase::Retired | SessionPhase::ToolExhausted
        ),
        Err(err) => {
            tracing::debug!(
                session_id,
                error = %err,
                "treating unreadable holder session as non-terminal"
            );
            false
        }
    }
}

fn holder_session_result_is_stale(project_root: &Path, session_id: &str) -> bool {
    const STALE_THRESHOLD: chrono::Duration = chrono::Duration::minutes(5);

    let session_dir = match csa_session::get_session_dir(project_root, session_id) {
        Ok(dir) => dir,
        Err(_) => return false,
    };
    let result_path = session_dir.join("result.toml");
    let result_mtime = match std::fs::metadata(&result_path).and_then(|meta| meta.modified()) {
        Ok(time) => time,
        Err(_) => return false,
    };

    let result_mtime: chrono::DateTime<chrono::Utc> = result_mtime.into();
    chrono::Utc::now().signed_duration_since(result_mtime) > STALE_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{ParentSessionSource, SessionCreationMode};
    use crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV;
    use crate::test_session_sandbox::ScopedSessionSandbox;
    use csa_core::types::ToolName;
    use csa_session::{PhaseEvent, ToolState, save_session};

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
        let _sandbox = ScopedSessionSandbox::new_blocking(&temp);
        let holder =
            csa_session::create_session_fresh(temp.path(), Some("done"), None, Some("codex"))
                .expect("create holder session");
        save_success_result(temp.path(), &holder.meta_session_id);
        let holder_lock = csa_lock::acquire_worktree_write_lock(
            temp.path(),
            &holder.meta_session_id,
            &[],
            |_| false,
            |_| false,
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
        let _sandbox = ScopedSessionSandbox::new_blocking(&temp);
        let holder =
            csa_session::create_session_fresh(temp.path(), Some("running"), None, Some("codex"))
                .expect("create holder session");
        let _holder_lock = csa_lock::acquire_worktree_write_lock(
            temp.path(),
            &holder.meta_session_id,
            &[],
            |_| false,
            |_| false,
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

    #[test]
    fn holder_session_is_terminal_when_phase_is_retired() {
        let temp = tempfile::tempdir().unwrap();
        let _sandbox = ScopedSessionSandbox::new_blocking(&temp);
        let mut holder =
            csa_session::create_session_fresh(temp.path(), Some("retired"), None, Some("codex"))
                .expect("create holder session");
        holder
            .apply_phase_event(PhaseEvent::Retired)
            .expect("holder should become Retired");
        save_session(&holder).expect("save Retired holder");

        assert!(super::holder_session_is_terminal(
            temp.path(),
            &holder.meta_session_id
        ));
    }

    #[test]
    fn holder_session_is_not_terminal_when_active_without_result() {
        let temp = tempfile::tempdir().unwrap();
        let _sandbox = ScopedSessionSandbox::new_blocking(&temp);
        let holder =
            csa_session::create_session_fresh(temp.path(), Some("running"), None, Some("codex"))
                .expect("create holder session");

        assert!(!super::holder_session_is_terminal(
            temp.path(),
            &holder.meta_session_id
        ));
    }

    #[test]
    fn holder_session_result_is_not_stale_when_missing() {
        let temp = tempfile::tempdir().unwrap();
        let _sandbox = ScopedSessionSandbox::new_blocking(&temp);
        let holder =
            csa_session::create_session_fresh(temp.path(), Some("running"), None, Some("codex"))
                .expect("create holder session");

        assert!(!super::holder_session_result_is_stale(
            temp.path(),
            &holder.meta_session_id
        ));
    }

    #[test]
    fn holder_session_result_is_not_stale_when_fresh() {
        let temp = tempfile::tempdir().unwrap();
        let _sandbox = ScopedSessionSandbox::new_blocking(&temp);
        let holder =
            csa_session::create_session_fresh(temp.path(), Some("done"), None, Some("codex"))
                .expect("create holder session");
        save_success_result(temp.path(), &holder.meta_session_id);

        assert!(!super::holder_session_result_is_stale(
            temp.path(),
            &holder.meta_session_id
        ));
    }

    #[test]
    #[cfg(unix)]
    fn holder_session_result_is_stale_after_threshold() {
        let temp = tempfile::tempdir().unwrap();
        let _sandbox = ScopedSessionSandbox::new_blocking(&temp);
        let holder =
            csa_session::create_session_fresh(temp.path(), Some("done"), None, Some("codex"))
                .expect("create holder session");
        save_success_result(temp.path(), &holder.meta_session_id);
        let session_dir = csa_session::get_session_dir(temp.path(), &holder.meta_session_id)
            .expect("session dir");
        // Backdate result.toml to simulate a holder whose transport completed
        // >5 minutes ago. Resumed sessions have their result.toml cleared
        // before acquiring the lock, so this correctly identifies a stuck
        // holder in the current execution.
        set_file_mtime_seconds_ago(&session_dir.join("result.toml"), 301);

        assert!(super::holder_session_result_is_stale(
            temp.path(),
            &holder.meta_session_id
        ));
    }

    #[tokio::test]
    async fn resumed_available_holder_persists_active_before_lineage_lock_check() {
        let temp = tempfile::tempdir().unwrap();
        let _sandbox = ScopedSessionSandbox::new(&temp).await;
        let project_root = temp.path();
        let mut holder =
            csa_session::create_session(project_root, Some("holder"), None, Some("codex"))
                .expect("create holder session");
        holder.tools.insert(
            "codex".to_string(),
            ToolState {
                provider_session_id: Some("provider-holder".to_string()),
                last_action_summary: String::new(),
                last_exit_code: 0,
                updated_at: chrono::Utc::now(),
                tool_version: Some("codex-test".to_string()),
                token_usage: None,
            },
        );
        holder
            .apply_phase_event(PhaseEvent::Compressed)
            .expect("holder should become Available");
        save_session(&holder).expect("save Available holder");
        let holder_session_id = holder.meta_session_id.clone();
        assert_eq!(
            csa_session::load_session(project_root, &holder_session_id)
                .expect("load saved holder")
                .phase,
            SessionPhase::Available
        );

        let bootstrapped = super::super::session_exec_bootstrap::bootstrap_session(
            &ToolName::Codex,
            "resume holder",
            Some(&holder_session_id),
            false,
            None,
            None,
            project_root,
            None,
            None,
            Some("run"),
            None,
            ParentSessionSource::ExplicitOrEnv,
            SessionCreationMode::DaemonManaged,
            &EMPTY_STARTUP_SUBTREE_ENV,
        )
        .await
        .expect("resume holder session");
        assert_eq!(bootstrapped.session.phase, SessionPhase::Active);
        assert_eq!(
            csa_session::load_session(project_root, &holder_session_id)
                .expect("load resumed holder")
                .phase,
            SessionPhase::Active,
            "resumed holder phase must be persisted before worktree-lock lineage checks"
        );

        let _holder_lock = acquire_if_needed(project_root, &bootstrapped.session, false)
            .expect("resumed holder should acquire worktree lock")
            .expect("writable holder should take a lock");
        let child = csa_session::create_session(
            project_root,
            Some("lineage child"),
            Some(&holder_session_id),
            Some("codex"),
        )
        .expect("create lineage child");

        let child_lock = acquire_if_needed(project_root, &child, false)
            .expect("child should re-enter under persisted-active holder")
            .expect("writable child should get a lock guard");

        assert!(child_lock.is_lineage_reentry());
        assert_eq!(
            child_lock.holder_session_id(),
            Some(holder_session_id.as_str())
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
                kill_hint: None,
                kill_diagnostics: None,
                last_item: None,
                fallback_chain: None,
                gate_timeout: false,
                warnings: Vec::new(),
                raw_process_exit_code: None,
                uncommitted_changes: None,
                large_diff_warning: None,
                require_commit_recovery: None,
                memory_soft_limit_recovery: None,
                post_exec_gate: None,
                manager_fields: Default::default(),
            },
        )
        .expect("save result");
    }

    #[cfg(unix)]
    fn set_file_mtime_seconds_ago(path: &Path, seconds_ago: u64) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before unix epoch");
        let target = now.saturating_sub(std::time::Duration::from_secs(seconds_ago));
        let tv_sec = target.as_secs() as libc::time_t;
        let tv_nsec = target.subsec_nanos() as libc::c_long;
        let times = [
            libc::timespec { tv_sec, tv_nsec },
            libc::timespec { tv_sec, tv_nsec },
        ];
        let c_path = CString::new(path.as_os_str().as_bytes()).expect("path contains NUL");
        // SAFETY: `utimensat` is called with a valid NUL-terminated path and
        // a stack-allocated timespec array.
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(rc, 0, "utimensat failed for {}", path.display());
    }
}
