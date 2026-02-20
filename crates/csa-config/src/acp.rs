use serde::{Deserialize, Serialize};

/// ACP transport-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpConfig {
    /// Timeout for ACP initialization/session setup operations.
    #[serde(default = "default_acp_init_timeout_seconds")]
    pub init_timeout_seconds: u64,
}

const fn default_acp_init_timeout_seconds() -> u64 {
    60
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            init_timeout_seconds: default_acp_init_timeout_seconds(),
        }
    }
}

impl AcpConfig {
    /// Returns true when all fields match defaults.
    pub fn is_default(&self) -> bool {
        self.init_timeout_seconds == default_acp_init_timeout_seconds()
    }
}
