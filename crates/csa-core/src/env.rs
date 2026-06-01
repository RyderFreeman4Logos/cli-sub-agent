/// Reserved executor env var that disables automatic runtime failover/retry paths.
pub const NO_FAILOVER_ENV_KEY: &str = "_CSA_NO_FAILOVER";

/// Exact model spec inherited by nested CSA invocations in a pinned SA subtree.
pub const CSA_MODEL_SPEC_ENV_KEY: &str = "CSA_MODEL_SPEC";

/// Whether nested CSA invocations should bypass tier settings for the inherited model spec.
pub const CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY: &str = "CSA_FORCE_IGNORE_TIER_SETTING";

/// Whether nested CSA invocations should disable failover for the inherited model spec.
pub const CSA_NO_FAILOVER_ENV_KEY: &str = "CSA_NO_FAILOVER";

/// Absolute path to the current session directory owned by this process.
pub const CSA_SESSION_DIR_ENV_KEY: &str = "CSA_SESSION_DIR";

/// Absolute path to the parent session directory when this process is a child session.
pub const CSA_PARENT_SESSION_DIR_ENV_KEY: &str = "CSA_PARENT_SESSION_DIR";
