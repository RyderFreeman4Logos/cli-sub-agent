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

/// Models accepted for each known tool.
///
/// This catalog covers the models used by current setup docs, global config
/// defaults, and common test fixtures. It is not intended to mirror every model
/// ever released by each provider.
pub fn valid_models(tool: &str) -> &'static [&'static str] {
    match tool {
        "codex" => &[
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
        ],
        "claude-code" => &[
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
        ],
        "gemini-cli" => &[
            "default",
            "gemini-3.1-pro-preview",
            "gemini-3.1-pro",
            "gemini-3-pro-preview",
            "gemini-3-pro",
            "gemini-3-flash-preview",
            "gemini-2.5-pro",
            "gemini-2.5-flash",
        ],
        "opencode" => &[
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-sonnet-4-5-20250929",
            "gpt-5.5",
            "gpt-5.4",
            "gemini-3.1-pro-preview",
            "gemini-2.5-pro",
        ],
        "openai-compat" => &[],
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
