pub const TOOL_NAME: &str = "gemini-cli";
pub const API_KEY_ENV: &str = "GEMINI_API_KEY";
pub const API_KEY_FALLBACK_ENV_KEY: &str = "_CSA_API_KEY_FALLBACK";
pub const NO_FLASH_FALLBACK_ENV_KEY: &str = "_CSA_NO_FLASH_FALLBACK";
pub const AUTH_MODE_ENV_KEY: &str = "_CSA_GEMINI_AUTH_MODE";
pub const AUTH_MODE_OAUTH: &str = "oauth";
pub const AUTH_MODE_API_KEY: &str = "api_key";
pub const BASE_URL_ENV: &str = "GOOGLE_GEMINI_BASE_URL";

/// Environment variables that gemini-cli reads for auth/routing.
/// CSA must strip these from the inherited process environment so that
/// auth mode is fully controlled by CSA's extra_env (OAuth-first with
/// API key fallback only after quota exhaustion).
pub const INHERITED_ENV_STRIP: &[&str] = &[API_KEY_ENV, BASE_URL_ENV];

pub const RATE_LIMIT_PATTERNS: &[&str] = &[
    "429",
    "resource exhausted",
    "resource_exhausted",
    "capacity exhausted",
    "capacity_exhausted",
    "exhausted your capacity",
    "no capacity available",
    "quota exhausted",
    "quota_exhausted",
    "quota exceeded",
    "too many requests",
];

pub const PERMANENT_QUOTA_EXHAUSTION_PATTERNS: &[&str] = &[
    "monthly spending cap",
    "monthly cap",
    "spending cap",
    "quota_exhausted_billing",
];

const PERMANENT_QUOTA_EXHAUSTION_CONTEXT_PATTERNS: &[&str] = &[
    "billing cap",
    "billing hard limit",
    "billing limit",
    "billing budget",
    "billing disabled",
    "billing is disabled",
    "billing is not enabled",
    "billing not enabled",
    "budget exceeded",
    "payment required",
    "spend limit",
    "spending limit",
];

const QUOTA_EXHAUSTED_REASON_PATTERNS: &[&str] = &["quota_exhausted", "quota exhausted"];

pub fn is_gemini_tool(tool: &str) -> bool {
    tool == TOOL_NAME
}

pub fn detect_rate_limit_pattern(output: &str) -> Option<&'static str> {
    let output_lower = output.to_ascii_lowercase();
    RATE_LIMIT_PATTERNS
        .iter()
        .copied()
        .find(|pattern| output_lower.contains(pattern))
}

pub fn detect_permanent_quota_exhaustion_pattern(output: &str) -> Option<&'static str> {
    let output_lower = output.to_ascii_lowercase();
    if let Some(pattern) = PERMANENT_QUOTA_EXHAUSTION_PATTERNS
        .iter()
        .copied()
        .find(|pattern| output_lower.contains(pattern))
    {
        return Some(pattern);
    }

    let has_quota_exhausted_reason = QUOTA_EXHAUSTED_REASON_PATTERNS
        .iter()
        .any(|pattern| output_lower.contains(pattern));
    if has_quota_exhausted_reason
        && PERMANENT_QUOTA_EXHAUSTION_CONTEXT_PATTERNS
            .iter()
            .any(|pattern| output_lower.contains(pattern))
    {
        return Some("quota_exhausted_billing");
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_gemini_permanent_quota_markers() {
        assert_eq!(
            detect_permanent_quota_exhaustion_pattern(
                "Gemini request failed because the monthly spending cap was reached"
            ),
            Some("monthly spending cap")
        );
        assert_eq!(
            detect_permanent_quota_exhaustion_pattern(
                "status: RESOURCE_EXHAUSTED; reason: QUOTA_EXHAUSTED; billing hard limit reached"
            ),
            Some("quota_exhausted_billing")
        );
        assert_eq!(
            detect_permanent_quota_exhaustion_pattern(
                "tool_exhausted: gemini-cli permanent quota exhaustion detected (matched 'quota_exhausted_billing')"
            ),
            Some("quota_exhausted_billing")
        );
    }

    #[test]
    fn does_not_treat_transient_quota_or_rate_limits_as_permanent_quota() {
        assert_eq!(
            detect_permanent_quota_exhaustion_pattern("HTTP 429 Too Many Requests"),
            None
        );
        assert_eq!(
            detect_permanent_quota_exhaustion_pattern("status: RESOURCE_EXHAUSTED"),
            None
        );
        assert_eq!(
            detect_permanent_quota_exhaustion_pattern(
                "status: RESOURCE_EXHAUSTED; reason: QUOTA_EXHAUSTED"
            ),
            None
        );
        assert_eq!(
            detect_permanent_quota_exhaustion_pattern("quota exceeded for model gemini-2.5-pro"),
            None
        );
        assert_eq!(
            detect_permanent_quota_exhaustion_pattern("daily quota limit reached"),
            None
        );
    }
}
