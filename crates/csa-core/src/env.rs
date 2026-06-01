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
/// which already requires the force-ignore marker + `ModelSpec::parse`, or by a
/// locally resolved spec), and [`SubtreeModelPin::pin_env_entries`] is the ONLY
/// function in the codebase that emits the pin keys for injection.
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
    /// reader (which gates on the paired force-ignore marker and `ModelSpec`
    /// well-formedness). A blank spec yields `None` (no pin).
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
