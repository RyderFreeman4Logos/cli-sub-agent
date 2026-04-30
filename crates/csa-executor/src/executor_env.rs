//! Environment ownership for child tool processes.

use std::collections::HashMap;
use std::ffi::OsStr;

use tokio::process::Command;

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

pub(crate) fn inject_git_guard_env(cmd: &mut Command) {
    let mut guard_env = HashMap::new();
    if let Some(path) = cmd
        .as_std()
        .get_envs()
        .find_map(|(key, value)| (key == OsStr::new("PATH")).then_some(value))
        .flatten()
    {
        guard_env.insert("PATH".to_string(), path.to_string_lossy().into_owned());
    }

    csa_hooks::git_guard::inject_git_guard_env(&mut guard_env);
    for key in ["PATH", "CSA_REAL_GIT"] {
        if let Some(value) = guard_env.get(key) {
            cmd.env(key, value);
        }
    }
}
