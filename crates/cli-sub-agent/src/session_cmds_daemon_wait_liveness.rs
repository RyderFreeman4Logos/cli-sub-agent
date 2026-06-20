use std::path::Path;

use crate::session_cmds_daemon::session_has_terminal_process;

pub(super) fn session_has_live_execution(
    worktree_lock_root: Option<&Path>,
    session_dir: &Path,
    resolved_session_id: &str,
    result_session_id: &str,
) -> bool {
    session_has_terminal_process(session_dir)
        || session_holds_worktree_write_lock(worktree_lock_root, result_session_id)
        || (resolved_session_id != result_session_id
            && session_holds_worktree_write_lock(worktree_lock_root, resolved_session_id))
}

pub(super) fn resume_handoff_blocks_target_reconcile(
    follows_resume_target: bool,
    wrapper_session_dir: &Path,
    target_session_dir: &Path,
) -> bool {
    follows_resume_target
        && crate::session_resume_handoff::resume_handoff_blocks_target_reconcile(
            wrapper_session_dir,
            target_session_dir,
        )
}

fn session_holds_worktree_write_lock(worktree_lock_root: Option<&Path>, session_id: &str) -> bool {
    let Some(worktree_lock_root) = worktree_lock_root else {
        return false;
    };

    match csa_lock::worktree_write_lock_is_held_by_session(worktree_lock_root, session_id) {
        Ok(held) => held,
        Err(error) => {
            tracing::debug!(
                session_id,
                worktree_lock_root = %worktree_lock_root.display(),
                error = %error,
                "failed to probe live worktree write lock for session wait"
            );
            false
        }
    }
}
