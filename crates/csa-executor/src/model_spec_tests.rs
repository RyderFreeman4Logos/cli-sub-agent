use super::*;

#[test]
fn test_parse_valid_spec() {
    let spec = ModelSpec::parse("opencode/google/gemini-2.5-pro/high").unwrap();
    assert_eq!(spec.tool, "opencode");
    assert_eq!(spec.provider, "google");
    assert_eq!(spec.model, "gemini-2.5-pro");
    assert!(matches!(spec.thinking_budget, ThinkingBudget::High));
}

#[test]
fn test_parse_spec_with_custom_budget() {
    let spec = ModelSpec::parse("codex/anthropic/claude-opus/5000").unwrap();
    assert_eq!(spec.tool, "codex");
    assert_eq!(spec.provider, "anthropic");
    assert_eq!(spec.model, "claude-opus");
    assert!(matches!(spec.thinking_budget, ThinkingBudget::Custom(5000)));
}

#[test]
fn test_parse_invalid_spec_wrong_parts() {
    let result = ModelSpec::parse("opencode/google/gemini");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("expected tool/provider/model/thinking_budget")
    );
}

#[test]
fn validate_with_catalog_accepts_known_spec() {
    let spec = ModelSpec::parse("codex/openai/gpt-5.5/xhigh").unwrap();
    assert!(
        spec.validate_with_catalog(
            &EffectiveModelCatalog::shipped().unwrap(),
            &["codex", "gemini-cli"]
        )
        .is_ok()
    );
}

#[test]
fn validate_with_catalog_preserves_configured_admission_warning() {
    let mut catalog = EffectiveModelCatalog::shipped().unwrap();
    catalog
        .register_configured_spec(
            "codex",
            "openai",
            "future-warning-model",
            "high",
            csa_core::model_catalog::CatalogProvenance::Inline {
                source: "test config".to_string(),
                key: "preferences.primary_writer_spec".to_string(),
            },
        )
        .unwrap();
    let spec = ModelSpec::parse("codex/openai/future-warning-model/high").unwrap();

    let admission = spec.validate_with_catalog(&catalog, &["codex"]).unwrap();

    let warning = admission
        .warning()
        .expect("configured warning must survive");
    assert!(warning.to_string().contains("future-warning-model"));
    assert!(
        warning
            .to_string()
            .contains("preferences.primary_writer_spec")
    );
}

#[test]
fn rejects_opencode_cross_provider_gemini_under_openai() {
    let spec = ModelSpec::parse("opencode/openai/gemini-2.5-pro/high").unwrap();
    let err = spec
        .validate_with_catalog(&EffectiveModelCatalog::shipped().unwrap(), &["opencode"])
        .unwrap_err();
    let message = err.to_string();

    assert!(message.contains("gemini-2.5-pro"));
    assert!(message.contains("openai"));
    assert!(message.contains("gpt-5"));
    assert!(!message.contains("claude-opus-4-7"));
}

#[test]
fn rejects_opencode_cross_provider_claude_under_google() {
    let spec = ModelSpec::parse("opencode/google/claude-opus-4-7/high").unwrap();
    let err = spec
        .validate_with_catalog(&EffectiveModelCatalog::shipped().unwrap(), &["opencode"])
        .unwrap_err();
    let message = err.to_string();

    assert!(message.contains("claude-opus-4-7"));
    assert!(message.contains("google"));
    assert!(message.contains("gemini-2.5-pro"));
    assert!(!message.contains("gpt-5"));
}

#[test]
fn accepts_opencode_correct_pairing() {
    for raw in [
        "opencode/openai/gpt-5/high",
        "opencode/google/gemini-2.5-pro/high",
        "opencode/anthropic/claude-opus-4-7/high",
    ] {
        let spec = ModelSpec::parse(raw).unwrap();
        assert!(
            spec.validate_with_catalog(&EffectiveModelCatalog::shipped().unwrap(), &["opencode"])
                .is_ok(),
            "{raw} should validate"
        );
    }
}

#[test]
fn validate_with_catalog_rejects_unknown_tool() {
    let spec = ModelSpec::parse("unknown/openai/gpt-5.5/xhigh").unwrap();
    let err = spec
        .validate_with_catalog(&EffectiveModelCatalog::shipped().unwrap(), &["codex"])
        .unwrap_err();
    assert!(err.to_string().contains("unknown"));
    assert!(err.to_string().contains("codex"));
}

#[test]
fn validate_with_catalog_rejects_catalog_known_tool_outside_caller_allowlist() {
    let spec = ModelSpec::parse("claude-code/anthropic/default/high").unwrap();
    let err = spec
        .validate_with_catalog(&EffectiveModelCatalog::shipped().unwrap(), &["codex"])
        .unwrap_err();
    assert!(err.to_string().contains("claude-code"));
    assert!(err.to_string().contains("codex"));
}

#[test]
fn validate_with_catalog_rejects_unknown_provider() {
    let spec = ModelSpec::parse("codex/anthropic/gpt-5.5/xhigh").unwrap();
    let err = spec
        .validate_with_catalog(&EffectiveModelCatalog::shipped().unwrap(), &["codex"])
        .unwrap_err();
    assert!(err.to_string().contains("anthropic"));
    assert!(err.to_string().contains("openai"));
}

#[test]
fn validate_with_catalog_rejects_unknown_model() {
    let spec = ModelSpec::parse("codex/openai/o3/xhigh").unwrap();
    let err = spec
        .validate_with_catalog(&EffectiveModelCatalog::shipped().unwrap(), &["codex"])
        .unwrap_err();
    assert!(err.to_string().contains("o3"));
    assert!(err.to_string().contains("gpt-5.5"));
}

#[test]
fn validate_with_catalog_skips_openai_compat_provider_and_model() {
    let spec = ModelSpec::parse("openai-compat/local/my-fine-tune/medium").unwrap();
    assert!(
        spec.validate_with_catalog(
            &EffectiveModelCatalog::shipped().unwrap(),
            &["openai-compat"]
        )
        .is_ok()
    );
}

#[test]
fn validate_with_catalog_skips_hermes_provider_and_model() {
    let spec = ModelSpec::parse("hermes/local/custom-model/xhigh").unwrap();
    assert!(
        spec.validate_with_catalog(&EffectiveModelCatalog::shipped().unwrap(), &["hermes"])
            .is_ok()
    );
}

#[test]
fn test_thinking_budget_parse_default() {
    assert!(matches!(
        ThinkingBudget::parse("default").unwrap(),
        ThinkingBudget::DefaultBudget
    ));
    assert!(matches!(
        ThinkingBudget::parse("Default").unwrap(),
        ThinkingBudget::DefaultBudget
    ));
    assert!(matches!(
        ThinkingBudget::parse("DEFAULT").unwrap(),
        ThinkingBudget::DefaultBudget
    ));
    assert!(matches!(
        ThinkingBudget::parse("none").unwrap(),
        ThinkingBudget::DefaultBudget
    ));
    assert!(matches!(
        ThinkingBudget::parse("None").unwrap(),
        ThinkingBudget::DefaultBudget
    ));
}

#[test]
fn test_thinking_budget_parse_low() {
    let budget = ThinkingBudget::parse("low").unwrap();
    assert!(matches!(budget, ThinkingBudget::Low));
}

#[test]
fn test_thinking_budget_parse_medium() {
    assert!(matches!(
        ThinkingBudget::parse("medium").unwrap(),
        ThinkingBudget::Medium
    ));
    assert!(matches!(
        ThinkingBudget::parse("med").unwrap(),
        ThinkingBudget::Medium
    ));
}

#[test]
fn test_thinking_budget_parse_high() {
    let budget = ThinkingBudget::parse("high").unwrap();
    assert!(matches!(budget, ThinkingBudget::High));
}

#[test]
fn test_thinking_budget_parse_xhigh() {
    assert!(matches!(
        ThinkingBudget::parse("xhigh").unwrap(),
        ThinkingBudget::Xhigh
    ));
    assert!(matches!(
        ThinkingBudget::parse("extra-high").unwrap(),
        ThinkingBudget::Xhigh
    ));
}

#[test]
fn test_thinking_budget_parse_custom() {
    let budget = ThinkingBudget::parse("1234").unwrap();
    assert!(matches!(budget, ThinkingBudget::Custom(1234)));
}

#[test]
fn test_thinking_budget_parse_invalid() {
    let result = ThinkingBudget::parse("invalid");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Invalid thinking budget")
    );
}

#[test]
fn test_thinking_budget_case_insensitive() {
    assert!(matches!(
        ThinkingBudget::parse("LOW").unwrap(),
        ThinkingBudget::Low
    ));
    assert!(matches!(
        ThinkingBudget::parse("High").unwrap(),
        ThinkingBudget::High
    ));
    assert!(matches!(
        ThinkingBudget::parse("XHIGH").unwrap(),
        ThinkingBudget::Xhigh
    ));
}

#[test]
fn test_thinking_budget_token_count() {
    assert_eq!(ThinkingBudget::DefaultBudget.token_count(), 10000);
    assert_eq!(ThinkingBudget::Low.token_count(), 1024);
    assert_eq!(ThinkingBudget::Medium.token_count(), 8192);
    assert_eq!(ThinkingBudget::High.token_count(), 32768);
    assert_eq!(ThinkingBudget::Xhigh.token_count(), 65536);
    assert_eq!(ThinkingBudget::Custom(5000).token_count(), 5000);
}

#[test]
fn test_thinking_budget_codex_effort() {
    assert_eq!(ThinkingBudget::DefaultBudget.codex_effort(), "medium");
    assert_eq!(ThinkingBudget::Low.codex_effort(), "low");
    assert_eq!(ThinkingBudget::Medium.codex_effort(), "medium");
    assert_eq!(ThinkingBudget::High.codex_effort(), "high");
    assert_eq!(ThinkingBudget::Xhigh.codex_effort(), "xhigh");
    assert_eq!(ThinkingBudget::Custom(10000).codex_effort(), "high"); // fallback to high
}

#[test]
fn test_thinking_budget_claude_effort() {
    // DefaultBudget = "let claude-code apply its own default" =>
    // omit the flag entirely (None).
    assert_eq!(ThinkingBudget::DefaultBudget.claude_effort(), None);
    assert_eq!(ThinkingBudget::Low.claude_effort(), Some("low"));
    assert_eq!(ThinkingBudget::Medium.claude_effort(), Some("medium"));
    assert_eq!(ThinkingBudget::High.claude_effort(), Some("high"));
    assert_eq!(ThinkingBudget::Xhigh.claude_effort(), Some("xhigh"));
    assert_eq!(ThinkingBudget::Max.claude_effort(), Some("max"));
    // Custom(n) has no level form in claude-code 2.x's --effort flag;
    // mirror codex_effort and pick "high" so the value stays accepted.
    assert_eq!(ThinkingBudget::Custom(10000).claude_effort(), Some("high"));
}

#[test]
fn try_split_provider_model_thinking() {
    let (model, budget) =
        ThinkingBudget::try_split_from_model("google/gemini-3.1-pro-preview/xhigh");
    assert_eq!(model, "google/gemini-3.1-pro-preview");
    assert!(matches!(budget, Some(ThinkingBudget::Xhigh)));
}

#[test]
fn try_split_model_thinking() {
    let (model, budget) = ThinkingBudget::try_split_from_model("gemini-3.1-pro-preview/high");
    assert_eq!(model, "gemini-3.1-pro-preview");
    assert!(matches!(budget, Some(ThinkingBudget::High)));
}

#[test]
fn try_split_no_thinking_suffix() {
    let (model, budget) = ThinkingBudget::try_split_from_model("google/gemini-3.1-pro-preview");
    assert_eq!(model, "google/gemini-3.1-pro-preview");
    assert!(budget.is_none());
}

#[test]
fn try_split_plain_model() {
    let (model, budget) = ThinkingBudget::try_split_from_model("gemini-3.1-pro-preview");
    assert_eq!(model, "gemini-3.1-pro-preview");
    assert!(budget.is_none());
}

#[test]
fn try_split_numeric_suffix_not_split() {
    // Numeric suffixes should NOT be treated as thinking budgets —
    // too ambiguous with model version numbers.
    let (model, budget) = ThinkingBudget::try_split_from_model("gpt-5.4/1000");
    assert_eq!(model, "gpt-5.4/1000");
    assert!(budget.is_none());
}

#[test]
fn try_split_case_insensitive() {
    let (model, budget) = ThinkingBudget::try_split_from_model("some-model/XHIGH");
    assert_eq!(model, "some-model");
    assert!(matches!(budget, Some(ThinkingBudget::Xhigh)));
}

#[test]
fn test_thinking_budget_parse_max() {
    assert!(matches!(
        ThinkingBudget::parse("max").unwrap(),
        ThinkingBudget::Max
    ));
    assert!(matches!(
        ThinkingBudget::parse("MAX").unwrap(),
        ThinkingBudget::Max
    ));
}

#[test]
fn test_thinking_budget_parse_error_mentions_max() {
    let err = ThinkingBudget::parse("invalid").unwrap_err().to_string();
    assert!(
        err.contains("max"),
        "error message should mention 'max': {err}"
    );
}

#[test]
fn test_thinking_budget_max_token_count() {
    assert_eq!(ThinkingBudget::Max.token_count(), 131072);
}

#[test]
fn test_thinking_budget_max_codex_effort() {
    assert_eq!(ThinkingBudget::Max.codex_effort(), "xhigh");
}

#[test]
fn try_split_max_suffix() {
    let (model, budget) = ThinkingBudget::try_split_from_model("some-model/max");
    assert_eq!(model, "some-model");
    assert!(matches!(budget, Some(ThinkingBudget::Max)));
}

#[test]
fn try_split_from_model_handles_max() {
    let (model, budget) = ThinkingBudget::try_split_from_model("gpt-5.4/max");
    assert_eq!(model, "gpt-5.4");
    assert!(matches!(budget, Some(ThinkingBudget::Max)));
}

#[test]
fn codex_stall_retry_preserves_configured_effort() {
    assert!(matches!(
        ThinkingBudget::Max.codex_stall_retry_budget(),
        Some(ThinkingBudget::Max)
    ));
    assert!(matches!(
        ThinkingBudget::Xhigh.codex_stall_retry_budget(),
        Some(ThinkingBudget::Xhigh)
    ));
    for budget in [
        ThinkingBudget::High,
        ThinkingBudget::Medium,
        ThinkingBudget::Low,
        ThinkingBudget::DefaultBudget,
        ThinkingBudget::Custom(50000),
    ] {
        assert!(
            budget.codex_stall_retry_budget().is_none(),
            "expected no transport retry for {budget:?}"
        );
    }
}

#[test]
fn test_parse_spec_with_max_budget() {
    let spec = ModelSpec::parse("claude-code/anthropic/default/max").unwrap();
    assert_eq!(spec.tool, "claude-code");
    assert_eq!(spec.provider, "anthropic");
    assert_eq!(spec.model, "default");
    assert!(matches!(spec.thinking_budget, ThinkingBudget::Max));
}

#[test]
fn test_parse_spec_with_none_budget_uses_tool_default() {
    let spec = ModelSpec::parse("claude-code/anthropic/claude-sonnet-4-20250514/none").unwrap();
    assert_eq!(spec.tool, "claude-code");
    assert_eq!(spec.provider, "anthropic");
    assert_eq!(spec.model, "claude-sonnet-4-20250514");
    assert!(matches!(
        spec.thinking_budget,
        ThinkingBudget::DefaultBudget
    ));
    assert!(
        spec.validate_with_catalog(&EffectiveModelCatalog::shipped().unwrap(), &["claude-code"])
            .is_ok()
    );
    assert_eq!(spec.thinking_budget.claude_effort(), None);
}

#[test]
fn tombstoned_model_has_distinct_validation_error() {
    let catalog = EffectiveModelCatalog::from_toml_str(
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "retired"
enabled = false
"#,
        "disabled model test",
    )
    .unwrap();
    let spec = ModelSpec::parse("codex/openai/retired/high").unwrap();
    let error = spec
        .validate_with_catalog(&catalog, &["codex"])
        .unwrap_err();
    assert!(matches!(
        error,
        ModelSpecValidationError::DisabledModel { .. }
    ));
    assert!(error.to_string().contains("tombstone"));
}
