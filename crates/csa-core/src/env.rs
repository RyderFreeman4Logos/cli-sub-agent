/// Reserved executor env var that disables automatic runtime failover/retry paths.
pub const NO_FAILOVER_ENV_KEY: &str = "_CSA_NO_FAILOVER";

/// Absolute path to the current session directory owned by this process.
pub const CSA_SESSION_DIR_ENV_KEY: &str = "CSA_SESSION_DIR";

/// Absolute path to the parent session directory when this process is a child session.
pub const CSA_PARENT_SESSION_DIR_ENV_KEY: &str = "CSA_PARENT_SESSION_DIR";
