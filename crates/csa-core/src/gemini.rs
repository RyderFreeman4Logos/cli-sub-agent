pub const TOOL_NAME: &str = "gemini-cli";
pub const API_KEY_ENV: &str = "GEMINI_API_KEY";
pub const API_KEY_FALLBACK_ENV_KEY: &str = "_CSA_API_KEY_FALLBACK";
pub const NO_FLASH_FALLBACK_ENV_KEY: &str = "_CSA_NO_FLASH_FALLBACK";
pub const AUTH_MODE_ENV_KEY: &str = "_CSA_GEMINI_AUTH_MODE";
pub const AUTH_MODE_OAUTH: &str = "oauth";
pub const AUTH_MODE_API_KEY: &str = "api_key";

pub const RATE_LIMIT_PATTERNS: &[&str] = &[
    "429",
    "resource exhausted",
    "resource_exhausted",
    "quota exhausted",
    "quota_exhausted",
    "quota exceeded",
    "too many requests",
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
