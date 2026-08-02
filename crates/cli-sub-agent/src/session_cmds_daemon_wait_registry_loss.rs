use std::path::Path;

pub(super) fn session_registry_state_loss(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
) -> bool {
    session_dir.is_dir() && csa_session::load_session(project_root, session_id).is_err()
}

pub(super) fn session_registry_phase_retired(project_root: &Path, session_id: &str) -> bool {
    csa_session::load_session(project_root, session_id)
        .is_ok_and(|session| matches!(session.phase, csa_session::SessionPhase::Retired))
}

pub(super) fn emit_registry_state_loss_or_missing_result(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
) {
    if !crate::session_observability::emit_session_registry_state_loss_diagnostic(
        project_root,
        session_id,
        session_dir,
    ) {
        eprintln!(
            "Session {session_id} has no readable registry state and no terminal result packet."
        );
        eprintln!("Run `csa session result --session {session_id}` for diagnostics.");
    }
}
