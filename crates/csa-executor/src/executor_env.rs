//! Environment ownership for child tool processes.

use std::collections::HashMap;
use std::ffi::OsStr;

use tokio::process::Command;

/// Variables scrubbed before tool spawn.
///
/// The list removes recursive-invocation guards, hook bypass switches,
/// session-scoped CSA values outside the startup subtree contract. The startup
/// subtree contract is scrubbed through `csa_core::env::scrub_subtree_contract_env_*`
/// so the key list has one source of truth (#1750).
pub const STRIPPED_ENV_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "LEFTHOOK",
    "LEFTHOOK_SKIP",
    "CSA_DAEMON_SESSION_DIR",
    csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY,
    csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY,
    csa_session::RESULT_TOML_PATH_CONTRACT_ENV,
];

/// Apply a CSA-decided subtree model pin to a child command (#1741).
///
/// This is the single executor-side writer of the subtree-pin env keys. It MUST
/// be called AFTER any generic env injection (which unconditionally strips the
/// pin keys via [`STRIPPED_ENV_VARS`] / [`csa_core::env::strip_reserved_pin_keys`])
/// so the trusted pin is the last writer and cannot be displaced by, or
/// forged from, user/request/config env. A `None` pin is a no-op, leaving the
/// keys env-removed (reserved) as the generic strip left them.
pub(crate) fn apply_subtree_pin(cmd: &mut Command, pin: Option<&csa_core::env::SubtreeModelPin>) {
    if let Some(pin) = pin {
        for (key, value) in pin.pin_env_entries() {
            cmd.env(key, value);
        }
    }
}

/// Apply CSA's explicit `git push` authorization to a child command.
///
/// Generic env maps and inherited process env are never trusted for this
/// authorization. Call after generic env injection so the typed decision is the
/// final writer of the leaf-tool contract key.
pub(crate) fn apply_git_push_authorization(cmd: &mut Command, allow_git_push: bool) {
    for key in csa_core::env::GIT_PUSH_AUTHORIZATION_ENV_KEYS {
        cmd.env_remove(key);
    }
    if allow_git_push {
        cmd.env(csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY, "true");
    }
}

pub(crate) fn inject_git_guard_env(cmd: &mut Command) {
    let mut guard_env = HashMap::new();
    for key in ["PATH", "CSA_SESSION_DIR"] {
        if let Some(value) = cmd
            .as_std()
            .get_envs()
            .find_map(|(env_key, value)| (env_key == OsStr::new(key)).then_some(value))
            .flatten()
        {
            guard_env.insert(key.to_string(), value.to_string_lossy().into_owned());
        }
    }

    csa_hooks::git_guard::inject_git_guard_env(&mut guard_env);
    for key in ["PATH", "CSA_REAL_GIT"] {
        if let Some(value) = guard_env.get(key) {
            cmd.env(key, value);
        }
    }
}
