use super::*;

#[test]
fn build_executor_full_spec_override_normalizes_codex_dispatch() {
    let executor = build_executor(
        &ToolName::Codex,
        Some("codex/openai/gpt-5.5/high"),
        Some("codex/openai/gpt-5.4/high"),
        None,
        None,
        false,
    )
    .expect("full Codex override");
    assert_eq!(executor.model_override(), Some("gpt-5.4"));
    assert_eq!(executor.thinking_budget(), Some(&ThinkingBudget::High));
}

#[test]
fn build_executor_full_spec_override_preserves_opencode_provider_dispatch() {
    let executor = build_executor(
        &ToolName::Opencode,
        Some("opencode/google/gemini-2.5-pro/high"),
        Some("opencode/anthropic/claude-sonnet-4-5/xhigh"),
        None,
        None,
        false,
    )
    .expect("full OpenCode override");
    assert_eq!(
        executor.model_override(),
        Some("anthropic/claude-sonnet-4-5")
    );
    assert_eq!(executor.thinking_budget(), Some(&ThinkingBudget::Xhigh));
}

#[test]
fn build_executor_full_spec_override_splits_hermes_provider_dispatch() {
    let executor = build_executor(
        &ToolName::Hermes,
        Some("hermes/openai/gpt-5.5/high"),
        Some("hermes/anthropic/claude-sonnet-4-5/low"),
        None,
        None,
        false,
    )
    .expect("full Hermes override");
    assert_eq!(executor.provider_override(), Some("anthropic"));
    assert_eq!(executor.model_override(), Some("claude-sonnet-4-5"));
    assert_eq!(executor.thinking_budget(), Some(&ThinkingBudget::Low));
}

#[test]
fn build_executor_alias_to_full_spec_override_uses_resolved_dispatch_identity() {
    let config: ProjectConfig = toml::from_str(
        r#"
[aliases]
future = "codex/openai/gpt-future/high"
"#,
    )
    .expect("alias config");
    let executor = build_executor(
        &ToolName::Codex,
        Some("codex/openai/gpt-current/low"),
        Some("future"),
        None,
        Some(&config),
        false,
    )
    .expect("full-spec alias override");
    assert_eq!(executor.model_override(), Some("gpt-future"));
    assert_eq!(executor.thinking_budget(), Some(&ThinkingBudget::High));
}
