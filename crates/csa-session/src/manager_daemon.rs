use std::path::Path;

use crate::state::MetaSessionState;
use anyhow::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionIdStrategy {
    DaemonAware(Option<String>),
    Fresh,
}

pub(crate) fn preassigned_daemon_session_id_from_env() -> Option<String> {
    let session_id = std::env::var(super::DAEMON_SESSION_ID_ENV)
        .ok()
        .filter(|value| !value.is_empty())?;
    let session_dir = std::env::var_os(super::DAEMON_SESSION_DIR_ENV);
    let project_root = std::env::var_os(super::DAEMON_PROJECT_ROOT_ENV);
    preassigned_daemon_session_id_from_values(
        Some(session_id.as_str()),
        session_dir.as_deref().map(Path::new),
        project_root.as_deref().map(Path::new),
    )
}

pub(crate) fn preassigned_daemon_session_id_from_values(
    session_id: Option<&str>,
    session_dir: Option<&Path>,
    project_root: Option<&Path>,
) -> Option<String> {
    let session_id = session_id.filter(|value| !value.is_empty())?;
    let has_daemon_context = session_dir.is_some() || project_root.is_some();
    has_daemon_context.then(|| session_id.to_string())
}

/// Create a session using explicit daemon env values rather than mutating the
/// process environment.
pub fn create_session_with_daemon_env(
    project_path: &Path,
    description: Option<&str>,
    parent_id: Option<&str>,
    tool: Option<&str>,
    daemon_session_id: Option<&str>,
    daemon_session_dir: Option<&Path>,
    daemon_project_root: Option<&Path>,
) -> Result<MetaSessionState> {
    let base_dir = super::get_session_root(project_path)?;
    super::create_session_in_with_strategy(
        &base_dir,
        project_path,
        description,
        parent_id,
        tool,
        SessionIdStrategy::DaemonAware(preassigned_daemon_session_id_from_values(
            daemon_session_id,
            daemon_session_dir,
            daemon_project_root,
        )),
    )
}

/// Resolved identifiers for resuming a tool session.
#[derive(Debug, Clone)]
pub struct ResumeSessionResolution {
    /// Fully resolved CSA meta session ID (ULID).
    pub meta_session_id: String,
    /// Provider-native session ID for the requested tool, if present in state.
    pub provider_session_id: Option<String>,
}
