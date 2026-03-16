use std::collections::HashMap;

use csa_core::gemini::{
    API_KEY_ENV, API_KEY_FALLBACK_ENV_KEY, AUTH_MODE_API_KEY, AUTH_MODE_ENV_KEY, AUTH_MODE_OAUTH,
    NO_FLASH_FALLBACK_ENV_KEY, is_gemini_tool,
};

use crate::global::GlobalConfig;

#[derive(Debug, Clone, Copy, Default)]
pub struct ExecutionEnvOptions {
    pub no_flash_fallback: bool,
}

impl ExecutionEnvOptions {
    pub const fn with_no_flash_fallback() -> Self {
        Self {
            no_flash_fallback: true,
        }
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

        if is_gemini_tool(tool) {
            if options.no_flash_fallback {
                env.insert(NO_FLASH_FALLBACK_ENV_KEY.to_string(), "1".to_string());
            }
            if let Some(key) = self.api_key_fallback(tool) {
                env.insert(API_KEY_FALLBACK_ENV_KEY.to_string(), key.to_string());
            }

            let auth_mode = if env.contains_key(API_KEY_ENV) {
                AUTH_MODE_API_KEY
            } else {
                AUTH_MODE_OAUTH
            };
            env.insert(AUTH_MODE_ENV_KEY.to_string(), auth_mode.to_string());
        }

        if env.is_empty() { None } else { Some(env) }
    }
}
