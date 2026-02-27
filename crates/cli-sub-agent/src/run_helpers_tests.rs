use super::{build_executor, infer_task_edit_requirement, resolve_tool, truncate_prompt};
use csa_config::GlobalConfig;
use csa_core::types::ToolName;

#[test]
fn truncate_prompt_short_string_unchanged() {
    assert_eq!(truncate_prompt("hello", 10), "hello");
}

#[test]
fn truncate_prompt_exact_length_unchanged() {
    assert_eq!(truncate_prompt("hello", 5), "hello");
}

#[test]
fn truncate_prompt_ascii_truncated() {
    let result = truncate_prompt("hello world this is long", 15);
    assert!(result.ends_with("..."));
    assert!(result.chars().count() <= 15);
}

#[test]
fn truncate_prompt_multibyte_no_panic() {
    // 10 CJK chars (3 bytes each = 30 bytes); truncate to 6 chars should not panic
    let cjk =
        "\u{4f60}\u{597d}\u{4e16}\u{754c}\u{6d4b}\u{8bd5}\u{8fd9}\u{662f}\u{4e2d}\u{6587}";
    let result = truncate_prompt(cjk, 6);
    assert!(result.ends_with("..."));
    assert!(result.chars().count() <= 6);
}

#[test]
fn truncate_prompt_emoji_no_panic() {
    let emoji = "Hello \u{1f30d}\u{1f525}\u{1f680} world test";
    let result = truncate_prompt(emoji, 10);
    assert!(result.ends_with("..."));
    assert!(result.chars().count() <= 10);
}

#[test]
fn truncate_prompt_empty_string() {
    assert_eq!(truncate_prompt("", 10), "");
}

#[test]
fn truncate_prompt_mixed_multibyte() {
    // Mix of ASCII, CJK, emoji
    let mixed = "Fix \u{4fee}\u{590d} bug \u{1f41b} in auth";
    let result = truncate_prompt(mixed, 12);
    assert!(result.ends_with("..."));
    assert!(result.chars().count() <= 12);
}

#[test]
fn infer_edit_requirement_detects_explicit_read_only() {
    let result = infer_task_edit_requirement("Analyze auth flow in read-only mode");
    assert_eq!(result, Some(false));
}

#[test]
fn infer_edit_requirement_detects_implementation_intent() {
    let result = infer_task_edit_requirement("Fix the login bug and update tests");
    assert_eq!(result, Some(true));
}

#[test]
fn infer_edit_requirement_read_only_overrides_edit_words() {
    let result = infer_task_edit_requirement("Do not edit files, only review this patch");
    assert_eq!(result, Some(false));
}

#[test]
fn infer_edit_requirement_returns_none_for_ambiguous_prompt() {
    let result = infer_task_edit_requirement("Continue work from previous session");
    assert_eq!(result, None);
}

#[test]
fn infer_edit_requirement_keeps_analysis_only_prompt_ambiguous() {
    let result = infer_task_edit_requirement("Review auth flow and report issues");
    assert_eq!(result, None);
}

#[test]
fn build_executor_model_and_thinking_coexist() {
    let exec = build_executor(
        &ToolName::Codex,
        None,
        Some("gpt-5.1-codex-mini"),
        Some("low"),
        None,
    )
    .unwrap();
    let debug = format!("{:?}", exec);
    assert!(
        debug.contains("gpt-5.1-codex-mini"),
        "model missing: {debug}"
    );
    assert!(debug.contains("Low"), "thinking budget missing: {debug}");
}

#[test]
fn build_executor_thinking_only() {
    let exec = build_executor(&ToolName::Codex, None, None, Some("high"), None).unwrap();
    let debug = format!("{:?}", exec);
    assert!(debug.contains("High"), "thinking budget missing: {debug}");
}

#[test]
fn build_executor_invalid_thinking_errors() {
    let result = build_executor(&ToolName::Codex, None, None, Some("bogus"), None);
    assert!(result.is_err());
}

// --- is_compress_command tests ---

#[test]
fn is_compress_command_slash_compress() {
    assert!(super::is_compress_command("/compress"));
}

#[test]
fn is_compress_command_slash_compact() {
    assert!(super::is_compress_command("/compact"));
}

#[test]
fn is_compress_command_slash_compact_with_args() {
    assert!(super::is_compress_command(
        "/compact Keep design decisions."
    ));
}

#[test]
fn is_compress_command_with_whitespace_padding() {
    assert!(super::is_compress_command("  /compress  "));
}

#[test]
fn is_compress_command_not_compress() {
    assert!(!super::is_compress_command("analyze the code"));
}

#[test]
fn is_compress_command_empty_string() {
    assert!(!super::is_compress_command(""));
}

#[test]
fn is_compress_command_partial_match_rejected() {
    assert!(!super::is_compress_command("/compressor"));
}

// --- parse_tool_name tests ---

#[test]
fn parse_tool_name_all_valid() {
    use super::parse_tool_name;
    assert!(matches!(
        parse_tool_name("gemini-cli").unwrap(),
        ToolName::GeminiCli
    ));
    assert!(matches!(
        parse_tool_name("opencode").unwrap(),
        ToolName::Opencode
    ));
    assert!(matches!(parse_tool_name("codex").unwrap(), ToolName::Codex));
    assert!(matches!(
        parse_tool_name("claude-code").unwrap(),
        ToolName::ClaudeCode
    ));
}

#[test]
fn resolve_tool_prefers_detected_value() {
    let mut config = GlobalConfig::default();
    config.defaults.tool = Some("claude-code".to_string());

    let resolved = resolve_tool(Some("codex".to_string()), &config);
    assert_eq!(resolved.as_deref(), Some("codex"));
}

#[test]
fn resolve_tool_uses_config_default_when_detection_missing() {
    let mut config = GlobalConfig::default();
    config.defaults.tool = Some("codex".to_string());

    let resolved = resolve_tool(None, &config);
    assert_eq!(resolved.as_deref(), Some("codex"));
}

#[test]
fn resolve_tool_returns_none_when_both_missing() {
    let config = GlobalConfig::default();
    let resolved = resolve_tool(None, &config);
    assert!(resolved.is_none());
}

#[test]
fn parse_tool_name_unknown_errors() {
    assert!(super::parse_tool_name("nvim").is_err());
}

#[test]
fn parse_tool_name_empty_errors() {
    assert!(super::parse_tool_name("").is_err());
}

// --- parse_token_usage tests ---

#[test]
fn parse_token_usage_all_fields() {
    let output = "input_tokens: 1000\noutput_tokens: 500\ntotal_tokens: 1500\ncost: $0.05";
    let usage = super::parse_token_usage(output).unwrap();
    assert_eq!(usage.input_tokens, Some(1000));
    assert_eq!(usage.output_tokens, Some(500));
    assert_eq!(usage.total_tokens, Some(1500));
    assert!((usage.estimated_cost_usd.unwrap() - 0.05).abs() < f64::EPSILON);
}

#[test]
fn parse_token_usage_input_output_sums_to_total() {
    // When only input_tokens and output_tokens are present (no explicit total),
    // total_tokens should be their sum. The generic "tokens:" pattern must NOT
    // match "output_tokens:" or "input_tokens:".
    let output = "input_tokens: 200\noutput_tokens: 300";
    let usage = super::parse_token_usage(output).unwrap();
    assert_eq!(usage.input_tokens, Some(200));
    assert_eq!(usage.output_tokens, Some(300));
    assert_eq!(usage.total_tokens, Some(500));
}

#[test]
fn parse_token_usage_explicit_total_preferred() {
    let output = "total_tokens: 1500";
    let usage = super::parse_token_usage(output).unwrap();
    assert_eq!(usage.total_tokens, Some(1500));
}

#[test]
fn parse_token_usage_generic_tokens_field() {
    let output = "Tokens: 5000";
    let usage = super::parse_token_usage(output).unwrap();
    assert_eq!(usage.total_tokens, Some(5000));
}

#[test]
fn parse_token_usage_no_match_returns_none() {
    let output = "Hello world, no token info here.";
    assert!(super::parse_token_usage(output).is_none());
}

#[test]
fn parse_token_usage_empty_string_returns_none() {
    assert!(super::parse_token_usage("").is_none());
}

// --- extract_number tests ---

#[test]
fn extract_number_basic() {
    assert_eq!(super::extract_number("tokens: 42"), Some(42));
}

#[test]
fn extract_number_with_spaces() {
    assert_eq!(super::extract_number("tokens:  123  "), Some(123));
}

#[test]
fn extract_number_no_colon_returns_none() {
    assert!(super::extract_number("tokens 42").is_none());
}

#[test]
fn extract_number_no_digits_returns_none() {
    assert!(super::extract_number("tokens: abc").is_none());
}

// --- extract_cost tests ---

#[test]
fn extract_cost_basic() {
    let result = super::extract_cost("cost: $1.50");
    assert!((result.unwrap() - 1.50).abs() < f64::EPSILON);
}

#[test]
fn extract_cost_small_value() {
    let result = super::extract_cost("estimated_cost: $0.0042");
    assert!((result.unwrap() - 0.0042).abs() < f64::EPSILON);
}

#[test]
fn extract_cost_no_dollar_returns_none() {
    assert!(super::extract_cost("cost: 1.50").is_none());
}

#[test]
fn extract_cost_empty_returns_none() {
    assert!(super::extract_cost("").is_none());
}

#[test]
fn build_executor_model_spec_overrides_both() {
    let exec = build_executor(
        &ToolName::Codex,
        Some("codex/openai/gpt-5.3-codex/xhigh"),
        Some("ignored-model"),
        Some("ignored-thinking"),
        None,
    )
    .unwrap();
    let debug = format!("{:?}", exec);
    assert!(
        debug.contains("gpt-5.3-codex"),
        "model_spec model missing: {debug}"
    );
    assert!(
        debug.contains("Xhigh"),
        "model_spec thinking missing: {debug}"
    );
}

// --- resolve_tool_and_model enablement guard tests ---

#[test]
fn resolve_tool_and_model_disabled_tool_explicit_errors() {
    use csa_config::{ProjectConfig, ToolConfig};
    use std::collections::HashMap;

    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: false,
            ..Default::default()
        },
    );

    let config = ProjectConfig {
        schema_version: 1,
        project: Default::default(),
        resources: Default::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
    };

    let result = super::resolve_tool_and_model(
        Some(ToolName::Codex),
        None,
        None,
        Some(&config),
        std::path::Path::new("/tmp"),
        true,  // force tier bypass
        false, // no override
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("disabled in user configuration"), "{msg}");
}

#[test]
fn resolve_tool_and_model_disabled_tool_with_override_succeeds() {
    use csa_config::{ProjectConfig, ToolConfig};
    use std::collections::HashMap;

    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: false,
            ..Default::default()
        },
    );

    let config = ProjectConfig {
        schema_version: 1,
        project: Default::default(),
        resources: Default::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
    };

    let result = super::resolve_tool_and_model(
        Some(ToolName::Codex),
        None,
        None,
        Some(&config),
        std::path::Path::new("/tmp"),
        true, // force tier bypass
        true, // override enabled
    );
    assert!(result.is_ok());
    let (tool, _, _) = result.unwrap();
    assert_eq!(tool, ToolName::Codex);
}

#[test]
fn resolve_tool_and_model_disabled_tool_model_spec_errors() {
    use csa_config::{ProjectConfig, ToolConfig};
    use std::collections::HashMap;

    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: false,
            ..Default::default()
        },
    );

    let config = ProjectConfig {
        schema_version: 1,
        project: Default::default(),
        resources: Default::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
    };

    let result = super::resolve_tool_and_model(
        None,
        Some("codex/openai/gpt-5.3-codex/high"),
        None,
        Some(&config),
        std::path::Path::new("/tmp"),
        true,
        false,
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("disabled in user configuration"), "{msg}");
}
