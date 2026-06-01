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
/// These are CSA-OWNED context vars: only CSA's own pin injection (when the
/// parent was explicitly `--model-spec`-pinned) may set them. Any value
/// inherited from the *ambient* process environment (a user's shell, a wrapper
/// script) MUST be reserved (cleared) at the child-spawn boundary so it can
/// never silently pin an otherwise-unpinned subtree and override tier routing
/// (#1741). This mirrors the `CLAUDECODE` recursion-guard reservation.
pub const SUBTREE_PIN_ENV_KEYS: &[&str] = &[
    CSA_MODEL_SPEC_ENV_KEY,
    CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
    CSA_NO_FAILOVER_ENV_KEY,
];

/// Absolute path to the current session directory owned by this process.
pub const CSA_SESSION_DIR_ENV_KEY: &str = "CSA_SESSION_DIR";

/// Absolute path to the parent session directory when this process is a child session.
pub const CSA_PARENT_SESSION_DIR_ENV_KEY: &str = "CSA_PARENT_SESSION_DIR";
