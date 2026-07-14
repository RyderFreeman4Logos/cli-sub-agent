/// Reserved executor env var that disables automatic runtime failover/retry paths.
pub const NO_FAILOVER_ENV_KEY: &str = "_CSA_NO_FAILOVER";

/// Cargo state/cache root.
pub const CARGO_HOME_ENV_KEY: &str = "CARGO_HOME";

/// Rustup state/toolchain root.
pub const RUSTUP_HOME_ENV_KEY: &str = "RUSTUP_HOME";

/// Cargo install root used by `cargo install`.
pub const CARGO_INSTALL_ROOT_ENV_KEY: &str = "CARGO_INSTALL_ROOT";

/// Cargo build artifact directory.
pub const CARGO_TARGET_DIR_ENV_KEY: &str = "CARGO_TARGET_DIR";

/// mise configuration directory.
pub const MISE_CONFIG_DIR_ENV_KEY: &str = "MISE_CONFIG_DIR";

/// mise data directory, which commonly contains installed toolchains.
pub const MISE_DATA_DIR_ENV_KEY: &str = "MISE_DATA_DIR";

/// Exact model spec inherited by nested CSA invocations in a pinned SA subtree.
pub const CSA_MODEL_SPEC_ENV_KEY: &str = "CSA_MODEL_SPEC";

/// Whether nested CSA invocations should bypass tier settings for the inherited model spec.
pub const CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY: &str = "CSA_FORCE_IGNORE_TIER_SETTING";

/// Whether nested CSA invocations should disable failover for the inherited model spec.
pub const CSA_NO_FAILOVER_ENV_KEY: &str = "CSA_NO_FAILOVER";

/// Current CSA session ULID inherited by a nested CSA process.
pub const CSA_SESSION_ID_ENV_KEY: &str = "CSA_SESSION_ID";

/// Current CSA recursion depth inherited by a nested CSA process.
pub const CSA_DEPTH_ENV_KEY: &str = "CSA_DEPTH";

/// Project root inherited by a nested CSA process.
pub const CSA_PROJECT_ROOT_ENV_KEY: &str = "CSA_PROJECT_ROOT";

/// Parent session ULID inherited by a nested CSA process.
pub const CSA_PARENT_SESSION_ENV_KEY: &str = "CSA_PARENT_SESSION";

/// Legacy parent-session key that must not leak into leaf provider processes.
pub const CSA_PARENT_SESSION_ID_ENV_KEY: &str = "CSA_PARENT_SESSION_ID";

/// Marker that a nested execution command was spawned by CSA itself.
pub const CSA_INTERNAL_INVOCATION_ENV_KEY: &str = "CSA_INTERNAL_INVOCATION";

/// Leaf-tool git wrapper authorization for `git push`.
///
/// This key is CSA-owned. Generic env maps and inherited process env MUST NOT
/// be trusted to set it; the executor may only write it from an explicit typed
/// run authorization.
pub const CSA_GIT_PUSH_ALLOWED_ENV_KEY: &str = "CSA_GIT_PUSH_ALLOWED";

/// Internal run authorization marker consumed before spawning a leaf tool.
///
/// This marker is never part of the child tool contract. It exists only as a
/// reserved key that must be scrubbed from generic env maps and inherited
/// process env.
pub const CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY: &str = "CSA_RUN_GIT_PUSH_AUTHORIZED";

/// Git-push authorization keys reserved for CSA-owned injection.
pub const GIT_PUSH_AUTHORIZATION_ENV_KEYS: &[&str] = &[
    CSA_GIT_PUSH_ALLOWED_ENV_KEY,
    CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY,
];

/// Strip git-push authorization keys from a generic env map.
pub fn strip_git_push_authorization_keys(env: &mut std::collections::HashMap<String, String>) {
    for key in GIT_PUSH_AUTHORIZATION_ENV_KEYS {
        env.remove(*key);
    }
}

/// Return true when a Rust toolchain state path should be overridden before
/// spawning a CSA session.
///
/// `/usr/local` is a tool-install prefix on the shared hosts CSA commonly runs
/// on, not a writable Cargo/Rustup state root. Descendants under `/usr/local`
/// are allowed only when the current namespace can write there; this preserves
/// intentionally mounted mise Rust homes such as
/// `/usr/local/share/mise/installs/rust/stable`.
pub fn rust_state_path_needs_session_override(path: &std::path::Path) -> bool {
    let usr_local = std::path::Path::new("/usr/local");
    if path == usr_local {
        return true;
    }
    path.starts_with(usr_local) && !rust_state_path_is_writable(path)
}

fn rust_state_path_is_writable(path: &std::path::Path) -> bool {
    let dir = if path.is_dir() {
        path
    } else if let Some(parent) = path.parent() {
        parent
    } else {
        return false;
    };
    if !dir.is_dir() {
        return false;
    }

    for attempt in 0..8 {
        let probe = dir.join(format!(
            ".csa-rust-env-probe-{}-{attempt}",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
        {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                return true;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return false,
        }
    }
    false
}

/// Marker that a CSA command runs INSIDE a weave pattern pipeline (`csa plan
/// run`).
///
/// `csa plan run` sets this to `"1"` on every `tool = "bash"` workflow step, and
/// it is inherited by every nested CSA process spawned from inside that subtree
/// (depth 0 → 1 → 2 …). When present (truthy), `csa run`/`review`/`debate`
/// DEFAULT the #1652 fatal-error-marker silent-hang scan to DISABLED: a
/// codex-fallback step legitimately multiplexes provider-error text into the
/// tool output stream (#1738/#1830) and would otherwise self-kill and abort the
/// whole pipeline (#1847). An explicit `--error-marker-scan` /
/// `--no-error-marker-scan` CLI flag still overrides this default.
///
/// Intentionally NOT part of [`STARTUP_SUBTREE_ENV_KEYS`]: suppressing a
/// heuristic early-kill is benign (idle-timeout and wall-clock timeout remain),
/// so the marker needs no spoof-resistance, and keeping it out of the scrubbed
/// contract is what lets it ride `build_merged_env` to leaf tools without being
/// stripped at the transport boundary. It propagates explicitly instead (plan
/// bash steps, `to_child_env_vars`, and the merged leaf-tool env).
pub const CSA_PATTERN_INTERNAL_ENV_KEY: &str = "CSA_PATTERN_INTERNAL";

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
/// Call this on ANY env map sourced from user/tool/ambient/request input so that
/// no user-controllable source can introduce a CSA-owned subtree pin (#1741).
/// Generic env-injection paths (every site that merges user/request/config env
/// into a spawned child) call this unconditionally; CSA's authoritative pin is
/// applied separately via the trusted typed [`SubtreeModelPin`] channel, never
/// through a generic env map.
pub fn strip_reserved_pin_keys(env: &mut std::collections::HashMap<String, String>) {
    for key in SUBTREE_PIN_ENV_KEYS {
        env.remove(*key);
    }
}

/// A CSA-decided subtree model pin, carried OUT-OF-BAND from any user/request
/// env map.
///
/// # Reservation invariant (#1741)
///
/// The subtree-pin env keys ([`SUBTREE_PIN_ENV_KEYS`]) reach a spawned child
/// **if and only if CSA itself decided to pin**. This type is the sole carrier
/// of that decision: it can only be built from validated CSA state (see
/// [`SubtreeModelPin::from_validated_spec`], fed by the inherited-pin reader
/// which already requires the force-ignore marker, `ModelSpec::parse`, and a
/// CSA-owned session sidecar check, or by a locally resolved spec), and
/// [`SubtreeModelPin::pin_env_entries`] is the ONLY function in the codebase
/// that emits the pin keys for injection.
///
/// Because the pin travels in this typed channel — never inside the generic
/// `extra_env` map that user/request/config input flows through (those are
/// unconditionally stripped via [`strip_reserved_pin_keys`]) — a caller placing
/// the pin keys in `SpawnConfig.env` / request env / config `extra_env` can
/// never spoof a pin. Spoof-resistance is true by construction, not by
/// enumerating each env-merge site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtreeModelPin {
    model_spec: String,
    no_failover: bool,
}

impl SubtreeModelPin {
    /// Construct a trusted pin from a CSA-validated model spec.
    ///
    /// `model_spec` MUST originate from validated CSA state: either a spec the
    /// current process resolved itself, or one returned by the inherited-pin
    /// reader (which gates on the paired force-ignore marker, `ModelSpec`
    /// well-formedness, and a CSA-owned session sidecar). A blank spec yields
    /// `None` (no pin).
    ///
    /// The pin always carries `CSA_FORCE_IGNORE_TIER_SETTING=1` because a CSA
    /// subtree pin is, by definition, a force-ignore-tier pin; `no_failover`
    /// records whether the pinned subtree also disables runtime failover.
    pub fn from_validated_spec(model_spec: &str, no_failover: bool) -> Option<Self> {
        let model_spec = model_spec.trim();
        if model_spec.is_empty() {
            return None;
        }
        Some(Self {
            model_spec: model_spec.to_string(),
            no_failover,
        })
    }

    /// The pinned model spec (`tool/provider/model/thinking`).
    pub fn model_spec(&self) -> &str {
        &self.model_spec
    }

    /// Whether the pinned subtree also disables runtime failover.
    pub fn no_failover(&self) -> bool {
        self.no_failover
    }

    /// The authoritative `(key, value)` env entries for this pin.
    ///
    /// This is the ONLY function that emits [`SUBTREE_PIN_ENV_KEYS`] for
    /// injection into a child process. Injection sites MUST apply these AFTER
    /// merging (and stripping) any generic env map, so the trusted pin is the
    /// last writer and cannot be displaced or forged.
    pub fn pin_env_entries(&self) -> Vec<(&'static str, String)> {
        let mut entries = vec![
            (CSA_MODEL_SPEC_ENV_KEY, self.model_spec.clone()),
            (CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1".to_string()),
        ];
        if self.no_failover {
            entries.push((CSA_NO_FAILOVER_ENV_KEY, "1".to_string()));
        }
        entries
    }
}

/// Absolute path to the current session directory owned by this process.
pub const CSA_SESSION_DIR_ENV_KEY: &str = "CSA_SESSION_DIR";

/// Absolute path to the parent session directory when this process is a child session.
pub const CSA_PARENT_SESSION_DIR_ENV_KEY: &str = "CSA_PARENT_SESSION_DIR";

/// CSA-owned subtree context env keys captured at CLI startup.
///
/// These keys describe the caller/session subtree and the trusted model-pin
/// reservation. A child process may receive them from its CSA parent. The CLI
/// freezes them into its startup snapshot, and leaf tool subprocess builders
/// scrub ambient copies before applying fresh per-session values.
pub const STARTUP_SUBTREE_ENV_KEYS: &[&str] = &[
    CSA_SESSION_ID_ENV_KEY,
    CSA_DEPTH_ENV_KEY,
    CSA_PROJECT_ROOT_ENV_KEY,
    CSA_SESSION_DIR_ENV_KEY,
    CSA_PARENT_SESSION_ENV_KEY,
    CSA_PARENT_SESSION_ID_ENV_KEY,
    CSA_PARENT_SESSION_DIR_ENV_KEY,
    CSA_INTERNAL_INVOCATION_ENV_KEY,
    CSA_MODEL_SPEC_ENV_KEY,
    CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
    CSA_NO_FAILOVER_ENV_KEY,
];

/// Return true when `key` belongs to the startup subtree contract.
pub fn is_startup_subtree_env_key(key: &str) -> bool {
    STARTUP_SUBTREE_ENV_KEYS.contains(&key)
}

/// Remove every startup subtree-contract key from a std process command.
pub fn scrub_subtree_contract_env(cmd: &mut std::process::Command) {
    for key in STARTUP_SUBTREE_ENV_KEYS {
        cmd.env_remove(key);
    }
}

/// Remove every startup subtree-contract key from a Tokio process command.
pub fn scrub_subtree_contract_env_tokio(cmd: &mut tokio::process::Command) {
    for key in STARTUP_SUBTREE_ENV_KEYS {
        cmd.env_remove(key);
    }
}

/// Remove every startup subtree-contract key from a generic env map.
pub fn scrub_subtree_contract_env_map(env: &mut std::collections::HashMap<String, String>) {
    for key in STARTUP_SUBTREE_ENV_KEYS {
        env.remove(*key);
    }
}

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

    #[test]
    fn rust_state_path_exact_usr_local_needs_session_override() {
        assert!(rust_state_path_needs_session_override(
            std::path::Path::new("/usr/local")
        ));
    }

    #[test]
    fn rust_state_path_outside_usr_local_does_not_need_session_override() {
        assert!(!rust_state_path_needs_session_override(
            std::path::Path::new("/tmp/csa-rust-home")
        ));
    }

    #[test]
    fn scrub_subtree_contract_env_map_removes_full_startup_contract() {
        let mut env = HashMap::from([
            (CSA_SESSION_ID_ENV_KEY.to_string(), "01KSESSION".to_string()),
            (
                CSA_PARENT_SESSION_ID_ENV_KEY.to_string(),
                "01KLEGACYPARENT".to_string(),
            ),
            (CSA_DEPTH_ENV_KEY.to_string(), "7".to_string()),
            (CSA_PROJECT_ROOT_ENV_KEY.to_string(), "/repo".to_string()),
            (CSA_INTERNAL_INVOCATION_ENV_KEY.to_string(), "1".to_string()),
            (
                CSA_MODEL_SPEC_ENV_KEY.to_string(),
                "codex/openai/gpt-5.5/xhigh".to_string(),
            ),
            ("KEEP_ME".to_string(), "value".to_string()),
        ]);

        scrub_subtree_contract_env_map(&mut env);

        for key in STARTUP_SUBTREE_ENV_KEYS {
            assert!(
                !env.contains_key(*key),
                "startup subtree-contract key {key} must be scrubbed"
            );
        }
        assert!(
            !env.contains_key(CSA_PARENT_SESSION_ID_ENV_KEY),
            "legacy parent-session key must be scrubbed"
        );
        assert_eq!(env.get("KEEP_ME").map(String::as_str), Some("value"));
    }

    #[test]
    fn subtree_model_pin_from_blank_spec_is_none() {
        assert!(SubtreeModelPin::from_validated_spec("", false).is_none());
        assert!(SubtreeModelPin::from_validated_spec("   ", true).is_none());
    }

    #[test]
    fn subtree_model_pin_entries_always_include_force_ignore_marker() {
        let pin = SubtreeModelPin::from_validated_spec("codex/openai/gpt-5.5/xhigh", false)
            .expect("non-blank spec");
        let entries: std::collections::HashMap<&str, String> =
            pin.pin_env_entries().into_iter().collect();

        assert_eq!(
            entries.get(CSA_MODEL_SPEC_ENV_KEY).map(String::as_str),
            Some("codex/openai/gpt-5.5/xhigh")
        );
        assert_eq!(
            entries
                .get(CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY)
                .map(String::as_str),
            Some("1")
        );
        // no_failover false → the key must be absent (never emitted as "off").
        assert!(!entries.contains_key(CSA_NO_FAILOVER_ENV_KEY));
    }

    #[test]
    fn subtree_model_pin_entries_include_no_failover_when_set() {
        let pin = SubtreeModelPin::from_validated_spec("codex/openai/gpt-5.5/xhigh", true)
            .expect("non-blank spec");
        let entries: std::collections::HashMap<&str, String> =
            pin.pin_env_entries().into_iter().collect();

        assert_eq!(
            entries.get(CSA_NO_FAILOVER_ENV_KEY).map(String::as_str),
            Some("1")
        );
        assert!(pin.no_failover());
        assert_eq!(pin.model_spec(), "codex/openai/gpt-5.5/xhigh");
    }
}
