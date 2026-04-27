//! Static catalog of known tool/provider/model combinations for offline
//! `--model-spec` validation. Updated when new models are released.

/// Providers accepted for each known tool.
///
/// `openai-compat` intentionally returns an empty list because users define
/// provider names through their own endpoint configuration.
pub fn valid_providers(tool: &str) -> &'static [&'static str] {
    match tool {
        "codex" => &["openai"],
        "claude-code" => &["anthropic"],
        "gemini-cli" => &["google"],
        "opencode" => &["openai", "google", "anthropic"],
        "openai-compat" => &[],
        _ => &[],
    }
}

const CODEX_OPENAI_MODELS: &[&str] = &[
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.3-codex",
    "gpt-5.3-codex-spark",
    "gpt-5-codex",
    "gpt-5-codex-mini",
    "gpt-5",
    "gpt-4o",
    "gpt-3.5-turbo",
];

const CLAUDE_CODE_ANTHROPIC_MODELS: &[&str] = &[
    "default",
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-sonnet-4-6",
    "claude-sonnet-4-5-20251001",
    "claude-sonnet-4-5-20250929",
    "claude-haiku-4-5-20251001",
    "claude-sonnet-4-20250514",
    "sonnet-4.6",
    "sonnet-4.5",
    "sonnet",
    "claude-sonnet",
    "opus",
    "claude-opus",
];

const GEMINI_CLI_GOOGLE_MODELS: &[&str] = &[
    "default",
    "gemini-3.1-pro-preview",
    "gemini-3.1-pro",
    "gemini-3-pro-preview",
    "gemini-3-pro",
    "gemini-3-flash-preview",
    "gemini-2.5-pro",
    "gemini-2.5-flash",
];

const OPENCODE_OPENAI_MODELS: &[&str] = &["gpt-5.5", "gpt-5.4", "gpt-5"];

const OPENCODE_GOOGLE_MODELS: &[&str] = &["gemini-3.1-pro-preview", "gemini-2.5-pro"];

const OPENCODE_ANTHROPIC_MODELS: &[&str] = &[
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-sonnet-4-6",
    "claude-sonnet-4-5-20250929",
];

/// Models accepted for each known tool/provider pair.
///
/// This catalog covers the models used by current setup docs, global config
/// defaults, and common test fixtures. It is not intended to mirror every model
/// ever released by each provider.
pub fn valid_models(tool: &str, provider: &str) -> &'static [&'static str] {
    match (tool, provider) {
        ("codex", "openai") => CODEX_OPENAI_MODELS,
        ("claude-code", "anthropic") => CLAUDE_CODE_ANTHROPIC_MODELS,
        ("gemini-cli", "google") => GEMINI_CLI_GOOGLE_MODELS,
        ("opencode", "openai") => OPENCODE_OPENAI_MODELS,
        ("opencode", "google") => OPENCODE_GOOGLE_MODELS,
        ("opencode", "anthropic") => OPENCODE_ANTHROPIC_MODELS,
        ("openai-compat", _) => &[],
        _ => &[],
    }
}

/// Whether provider validation is meaningful for this tool.
pub fn provider_validation_enabled(tool: &str) -> bool {
    !matches!(tool, "openai-compat")
}

/// Whether model validation is meaningful for this tool.
pub fn model_validation_enabled(tool: &str) -> bool {
    !matches!(tool, "openai-compat")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_models_are_provider_scoped() {
        assert!(valid_models("opencode", "openai").contains(&"gpt-5"));
        assert!(!valid_models("opencode", "openai").contains(&"gemini-2.5-pro"));
        assert!(valid_models("opencode", "google").contains(&"gemini-2.5-pro"));
        assert!(!valid_models("opencode", "google").contains(&"claude-opus-4-7"));
        assert!(valid_models("opencode", "anthropic").contains(&"claude-opus-4-7"));
        assert!(!valid_models("opencode", "anthropic").contains(&"gpt-5"));
    }

    #[test]
    fn single_provider_tools_keep_existing_model_lists() {
        assert!(valid_models("codex", "openai").contains(&"gpt-5"));
        assert!(valid_models("claude-code", "anthropic").contains(&"default"));
        assert!(valid_models("gemini-cli", "google").contains(&"gemini-2.5-pro"));
    }
}
