//! Environment ownership for child tool processes.

/// Variables scrubbed before tool spawn.
///
/// The list removes recursive-invocation guards, hook bypass switches, and
/// session-scoped CSA values that must be rebuilt for each fresh session.
pub(crate) const STRIPPED_ENV_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "LEFTHOOK",
    "LEFTHOOK_SKIP",
    "CSA_SESSION_ID",
    "CSA_SESSION_DIR",
    "CSA_PARENT_SESSION",
    "CSA_PARENT_SESSION_DIR",
    "CSA_DAEMON_SESSION_DIR",
    csa_session::RESULT_TOML_PATH_CONTRACT_ENV,
];
