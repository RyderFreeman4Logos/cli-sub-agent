//! Session directory resolution for sandbox plan construction.

use std::path::{Path, PathBuf};

pub(super) fn resolve_session_dir_for_sandbox(project_root: &Path, session_id: &str) -> PathBuf {
    csa_session::manager::get_session_dir(project_root, session_id).unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("cli-sub-agent")
            .join("sessions")
            .join(session_id)
    })
}
