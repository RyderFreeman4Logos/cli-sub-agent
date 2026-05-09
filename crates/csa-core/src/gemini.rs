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
    "429_quota_exhausted",
    "quota_exhausted",
    "quota exhausted",
    "monthly spending cap",
    "monthly cap",
    "spending cap",
    "daily quota",
    "quota exceeded",
];

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
    PERMANENT_QUOTA_EXHAUSTION_PATTERNS
        .iter()
        .copied()
        .find(|pattern| output_lower.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_gemini_permanent_quota_markers() {
        assert_eq!(
            detect_permanent_quota_exhaustion_pattern(
                "status: RESOURCE_EXHAUSTED; reason: QUOTA_EXHAUSTED"
            ),
            Some("quota_exhausted")
        );
        assert_eq!(
            detect_permanent_quota_exhaustion_pattern(
                "Gemini request failed because the monthly spending cap was reached"
            ),
            Some("monthly spending cap")
        );
    }

    #[test]
    fn does_not_treat_plain_429_as_permanent_quota() {
        assert_eq!(
            detect_permanent_quota_exhaustion_pattern("HTTP 429 Too Many Requests"),
            None
        );
        assert_eq!(
            detect_permanent_quota_exhaustion_pattern("status: RESOURCE_EXHAUSTED"),
            None
        );
    }
}
