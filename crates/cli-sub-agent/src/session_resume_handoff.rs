use std::path::Path;

/// Return true while a resume wrapper still owns the handoff to a target session.
///
/// During `--session` resume bootstrap, `resume-target.toml` is written before the
/// resumed target necessarily has its own PID, worktree lock, output, or result.
/// In that window the wrapper daemon is still the ownership signal; callers must
/// not synthesize a target failure from target-local silence alone.
pub(crate) fn resume_handoff_blocks_target_reconcile(
    wrapper_session_dir: &Path,
    target_session_dir: &Path,
) -> bool {
    !target_has_terminal_result(target_session_dir)
        && !target_has_reconcile_liveness_signal(target_session_dir)
        && wrapper_still_owns_handoff(wrapper_session_dir)
}

fn target_has_terminal_result(target_session_dir: &Path) -> bool {
    target_session_dir
        .join(csa_session::result::RESULT_FILE_NAME)
        .is_file()
}

fn target_has_reconcile_liveness_signal(target_session_dir: &Path) -> bool {
    crate::session_cmds_reconcile_liveness::reconcile_liveness_decision(target_session_dir)
        .blocks_synthesis
}

fn wrapper_still_owns_handoff(wrapper_session_dir: &Path) -> bool {
    if wrapper_completion_is_terminal(wrapper_session_dir) {
        return false;
    }

    wrapper_has_process_signal(wrapper_session_dir)
        || csa_process::ToolLiveness::is_alive_read_only(wrapper_session_dir)
}

fn wrapper_completion_is_terminal(wrapper_session_dir: &Path) -> bool {
    let Some(packet) =
        crate::session_cmds_daemon::load_daemon_completion_packet(wrapper_session_dir)
            .ok()
            .flatten()
    else {
        return false;
    };

    packet.is_legacy_complete_marker() || !wrapper_has_process_signal(wrapper_session_dir)
}

fn wrapper_has_process_signal(wrapper_session_dir: &Path) -> bool {
    csa_process::ToolLiveness::has_live_process(wrapper_session_dir)
        || csa_process::ToolLiveness::daemon_pid_is_alive(wrapper_session_dir)
}
