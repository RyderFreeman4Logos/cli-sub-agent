/// Reserved executor env var that disables automatic runtime failover/retry paths.
pub const NO_FAILOVER_ENV_KEY: &str = "_CSA_NO_FAILOVER";

/// Exact model spec inherited by nested CSA invocations in a pinned SA subtree.
pub const CSA_MODEL_SPEC_ENV_KEY: &str = "CSA_MODEL_SPEC";

/// Whether nested CSA invocations should bypass tier settings for the inherited model spec.
pub const CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY: &str = "CSA_FORCE_IGNORE_TIER_SETTING";

/// Whether nested CSA invocations should disable failover for the inherited model spec.
pub const CSA_NO_FAILOVER_ENV_KEY: &str = "CSA_NO_FAILOVER";

/// Subtree model-pin env vars that propagate a pinned SA subtree to nested
/// workers.
///
/// These are CSA-OWNED context vars governed by ONE reservation invariant: they
/// may ONLY be set by CSA's own pin injection (`inject_subtree_model_pin_env`,
/// run when the parent was explicitly `--model-spec`-pinned), and MUST NEVER be
/// honored from any user-controllable source. Concretely:
///
///   * ambient process environment (a user's shell, a wrapper script) — cleared
///     at the child-spawn boundary, mirroring the `CLAUDECODE` reservation;
///   * tool-config `[tools.<name>].extra_env` (and any other user env map) —
///     stripped via [`strip_reserved_pin_keys`] before CSA injects its own pin.
///
/// The distinguishing rule is the SOURCE, not the value: every user/ambient
/// source is filtered unconditionally; only CSA's dedicated injection writes
/// these keys. Without this, user config could spoof a CSA-owned subtree pin
/// and silently override tier routing (#1741).
pub const SUBTREE_PIN_ENV_KEYS: &[&str] = &[
    CSA_MODEL_SPEC_ENV_KEY,
    CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
    CSA_NO_FAILOVER_ENV_KEY,
];

/// Strip the reserved subtree model-pin keys from a user-supplied env map.
///
/// Call this on ANY env map sourced from user/tool/ambient input BEFORE CSA
/// injects its authoritative pin, so that no user-controllable source can spoof
/// a CSA-owned subtree pin (#1741). CSA's own injection runs afterwards and is
/// the only writer that survives.
pub fn strip_reserved_pin_keys(env: &mut std::collections::HashMap<String, String>) {
    for key in SUBTREE_PIN_ENV_KEYS {
        env.remove(*key);
    }
}

/// Absolute path to the current session directory owned by this process.
pub const CSA_SESSION_DIR_ENV_KEY: &str = "CSA_SESSION_DIR";

/// Absolute path to the parent session directory when this process is a child session.
pub const CSA_PARENT_SESSION_DIR_ENV_KEY: &str = "CSA_PARENT_SESSION_DIR";

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn strip_reserved_pin_keys_removes_all_reserved_and_keeps_others() {
        let mut env = HashMap::from([
            (
                CSA_MODEL_SPEC_ENV_KEY.to_string(),
                "codex/openai/gpt-5.5/xhigh".to_string(),
            ),
            (
                CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY.to_string(),
                "1".to_string(),
            ),
            (CSA_NO_FAILOVER_ENV_KEY.to_string(), "1".to_string()),
            ("KEEP_ME".to_string(), "value".to_string()),
        ]);

        strip_reserved_pin_keys(&mut env);

        for key in SUBTREE_PIN_ENV_KEYS {
            assert!(
                !env.contains_key(*key),
                "reserved key {key} must be stripped"
            );
        }
        assert_eq!(env.get("KEEP_ME").map(String::as_str), Some("value"));
    }
}
