use std::collections::HashMap;

use csa_core::{
    env::NO_FAILOVER_ENV_KEY,
    gemini::{
        API_KEY_ENV, API_KEY_FALLBACK_ENV_KEY, AUTH_MODE_ENV_KEY, AUTH_MODE_OAUTH,
        NO_FLASH_FALLBACK_ENV_KEY, is_gemini_tool,
    },
};

use crate::global::GlobalConfig;

#[derive(Debug, Clone, Copy, Default)]
pub struct ExecutionEnvOptions {
    pub no_flash_fallback: bool,
    pub no_failover: bool,
}

impl ExecutionEnvOptions {
    pub const fn with_no_flash_fallback() -> Self {
        Self {
            no_flash_fallback: true,
            no_failover: false,
        }
    }

    pub const fn with_no_failover(mut self) -> Self {
        self.no_failover = true;
        self
    }

    pub const fn from_no_failover(no_failover: bool) -> Self {
        let mut opts = Self {
            no_flash_fallback: false,
            no_failover: false,
        };
        if no_failover {
            opts.no_failover = true;
        }
        opts
    }
}

impl GlobalConfig {
    /// Build execution environment for a tool, including Gemini-specific fallbacks.
    pub fn build_execution_env(
        &self,
        tool: &str,
        options: ExecutionEnvOptions,
    ) -> Option<HashMap<String, String>> {
        let mut env = self.env_vars(tool).cloned().unwrap_or_default();
        // Drop any user-configured attempt to spoof CSA-owned execution env.
        env.remove(NO_FAILOVER_ENV_KEY);
        env.remove(NO_FLASH_FALLBACK_ENV_KEY);

        if options.no_failover {
            env.insert(NO_FAILOVER_ENV_KEY.to_string(), "1".to_string());
        }

        if is_gemini_tool(tool) {
            let legacy_api_key = env.remove(API_KEY_ENV);
            if options.no_flash_fallback {
                env.insert(NO_FLASH_FALLBACK_ENV_KEY.to_string(), "1".to_string());
            }
            if let Some(key) = self.api_key_fallback(tool).or(legacy_api_key.as_deref()) {
                env.insert(API_KEY_FALLBACK_ENV_KEY.to_string(), key.to_string());
            }
            env.insert(AUTH_MODE_ENV_KEY.to_string(), AUTH_MODE_OAUTH.to_string());
        }

        if env.is_empty() { None } else { Some(env) }
    }
}
