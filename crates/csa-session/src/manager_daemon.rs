#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionIdStrategy {
    DaemonAware,
    Fresh,
}

pub(crate) fn preassigned_daemon_session_id() -> Option<String> {
    let session_id = std::env::var(super::DAEMON_SESSION_ID_ENV)
        .ok()
        .filter(|value| !value.is_empty())?;
    let has_daemon_context = std::env::var_os(super::DAEMON_SESSION_DIR_ENV).is_some()
        || std::env::var_os(super::DAEMON_PROJECT_ROOT_ENV).is_some();
    has_daemon_context.then_some(session_id)
}

/// Resolved identifiers for resuming a tool session.
#[derive(Debug, Clone)]
pub struct ResumeSessionResolution {
    /// Fully resolved CSA meta session ID (ULID).
    pub meta_session_id: String,
    /// Provider-native session ID for the requested tool, if present in state.
    pub provider_session_id: Option<String>,
}
