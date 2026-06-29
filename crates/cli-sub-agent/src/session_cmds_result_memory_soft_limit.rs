use crate::memory_soft_limit_recovery_display::{
    build_memory_soft_limit_recovery_guidance_for_display_session,
    format_memory_soft_limit_recovery_lines_for_display_session,
};
use std::path::Path;

pub(super) fn lines(
    session_id: &str,
    session_dir: &Path,
    envelope: &csa_session::SessionResult,
    recovery: &csa_session::MemorySoftLimitRecoveryDiagnostic,
) -> Vec<String> {
    format_memory_soft_limit_recovery_lines_for_display_session(
        session_dir,
        session_id,
        recovery,
        envelope.kill_diagnostics.as_ref(),
    )
}

pub(super) fn insert(
    payload: &mut serde_json::Value,
    session_id: &str,
    session_dir: &Path,
    envelope: &csa_session::SessionResult,
) {
    let Some(recovery) = envelope.memory_soft_limit_recovery.as_ref() else {
        return;
    };
    payload["memory_soft_limit_recovery_guidance"] =
        build_memory_soft_limit_recovery_guidance_for_display_session(
            session_dir,
            session_id,
            recovery,
            envelope.kill_diagnostics.as_ref(),
        )
        .to_json();
}
