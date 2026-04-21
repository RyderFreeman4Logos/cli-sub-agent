use super::*;

#[cfg(unix)]
#[tokio::test]
async fn tier_fallback_advances_across_tool_variants_when_explicit_tool_and_tier() {
    let mut config = project_config_with_enabled_tools(&["codex", "gemini-cli"]);
    config.tools.get_mut("codex").unwrap().transport = Some(csa_config::ToolTransport::Cli);
    config.tiers.insert(
        "quality".to_string(),
        csa_config::config::TierConfig {
            description: "quality".to_string(),
            models: vec![
                "codex/openai/gpt-5.4/medium".to_string(),
                "codex/openai/gpt-5/high".to_string(),
                "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
            ],
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    let candidates = crate::tier_model_fallback::ordered_tier_candidates(
        csa_core::types::ToolName::Codex,
        Some("codex/openai/gpt-5.4/medium"),
        Some("quality"),
        Some(&config),
        true,
        Some(&crate::tier_model_fallback::TierFilter::whitelist([
            "codex",
        ])),
    );

    assert_eq!(
        candidates,
        vec![
            (
                csa_core::types::ToolName::Codex,
                Some("codex/openai/gpt-5.4/medium".to_string()),
            ),
            (
                csa_core::types::ToolName::Codex,
                Some("codex/openai/gpt-5/high".to_string()),
            ),
        ]
    );
}
