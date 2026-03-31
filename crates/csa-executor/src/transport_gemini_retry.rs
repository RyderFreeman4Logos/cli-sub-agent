use std::collections::HashMap;
use std::time::Duration;

use csa_core::gemini::{
    API_KEY_ENV as GEMINI_API_KEY_ENV, API_KEY_FALLBACK_ENV_KEY, AUTH_MODE_API_KEY,
    AUTH_MODE_ENV_KEY as GEMINI_AUTH_MODE_ENV_KEY, AUTH_MODE_OAUTH, NO_FLASH_FALLBACK_ENV_KEY,
    detect_rate_limit_pattern,
};
use csa_process::ExecutionResult;

pub(crate) const GEMINI_RATE_LIMIT_MAX_ATTEMPTS: u8 = 3;
pub(crate) const GEMINI_RATE_LIMIT_NO_FLASH_ATTEMPTS: u8 = 2;
#[cfg(test)]
pub(crate) const GEMINI_RATE_LIMIT_BASE_BACKOFF_MS: u64 = 10;
#[cfg(not(test))]
pub(crate) const GEMINI_RATE_LIMIT_BASE_BACKOFF_MS: u64 = 1_000;
pub(crate) const GEMINI_RATE_LIMIT_RETRY_MODEL_FIRST: &str = "gemini-3.1-pro-preview";
pub(crate) const GEMINI_RATE_LIMIT_RETRY_MODEL_SECOND: &str = "gemini-3-flash-preview";

pub(crate) fn gemini_is_no_flash(extra_env: Option<&HashMap<String, String>>) -> bool {
    extra_env.is_some_and(|env| env.contains_key(NO_FLASH_FALLBACK_ENV_KEY))
}

pub(crate) fn gemini_rate_limit_backoff(attempt: u8) -> Duration {
    let exponent = u32::from(attempt.saturating_sub(1));
    let multiplier = 1u64.checked_shl(exponent).unwrap_or(u64::MAX);
    Duration::from_millis(GEMINI_RATE_LIMIT_BASE_BACKOFF_MS.saturating_mul(multiplier))
}

pub(crate) fn gemini_retry_model(attempt: u8) -> Option<&'static str> {
    match attempt {
        2 => Some(GEMINI_RATE_LIMIT_RETRY_MODEL_FIRST),
        3 => Some(GEMINI_RATE_LIMIT_RETRY_MODEL_SECOND),
        _ => None,
    }
}

pub(crate) fn gemini_max_attempts(extra_env: Option<&HashMap<String, String>>) -> u8 {
    if gemini_is_no_flash(extra_env) {
        GEMINI_RATE_LIMIT_NO_FLASH_ATTEMPTS
    } else {
        GEMINI_RATE_LIMIT_MAX_ATTEMPTS
    }
}

pub(crate) fn is_gemini_rate_limited_result(execution: &ExecutionResult) -> bool {
    if execution.exit_code == 0 {
        return false;
    }
    detect_rate_limit_pattern(&format!(
        "{}\n{}",
        execution.stderr_output, execution.output
    ))
    .is_some()
}

pub(crate) fn is_gemini_rate_limited_error(error_msg: &str) -> bool {
    detect_rate_limit_pattern(error_msg).is_some()
}

pub(crate) fn gemini_auth_mode(extra_env: Option<&HashMap<String, String>>) -> Option<&str> {
    extra_env
        .and_then(|env| env.get(GEMINI_AUTH_MODE_ENV_KEY))
        .map(String::as_str)
}

/// Build extra_env with GEMINI_API_KEY injected from the fallback key.
/// Returns None if no fallback key is available or auth mode is not OAuth.
pub(crate) fn gemini_inject_api_key_fallback(
    extra_env: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    if gemini_auth_mode(extra_env) != Some(AUTH_MODE_OAUTH) {
        return None;
    }
    let fallback_key = extra_env?.get(API_KEY_FALLBACK_ENV_KEY)?;
    let mut env = extra_env.cloned().unwrap_or_default();
    env.insert(GEMINI_API_KEY_ENV.to_string(), fallback_key.clone());
    env.insert(
        GEMINI_AUTH_MODE_ENV_KEY.to_string(),
        AUTH_MODE_API_KEY.to_string(),
    );
    env.remove(API_KEY_FALLBACK_ENV_KEY);
    Some(env)
}
