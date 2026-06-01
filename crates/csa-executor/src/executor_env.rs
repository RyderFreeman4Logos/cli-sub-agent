//! Environment ownership for child tool processes.

use std::collections::HashMap;
use std::ffi::OsStr;

use tokio::process::Command;

/// Variables scrubbed before tool spawn.
///
/// The list removes recursive-invocation guards, hook bypass switches,
/// session-scoped CSA values that must be rebuilt for each fresh session, and
/// the subtree model-pin context vars. The latter are CSA-owned: any value
/// inherited from the *ambient* process environment must be cleared here so it
/// can never silently pin an otherwise-unpinned subtree; CSA re-injects them
/// (via `inject_subtree_model_pin_env` → `extra_env`) only when the parent was
/// explicitly `--model-spec`-pinned (#1741).
pub const STRIPPED_ENV_VARS: &[&str] = &[
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
    csa_core::env::CSA_MODEL_SPEC_ENV_KEY,
    csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
    csa_core::env::CSA_NO_FAILOVER_ENV_KEY,
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
