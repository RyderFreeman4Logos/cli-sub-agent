use super::*;
use std::str::FromStr;

#[test]
fn test_tool_arg_from_str_auto() {
    let arg = ToolArg::from_str("auto").unwrap();
    assert!(matches!(arg, ToolArg::Auto));
}

#[test]
fn test_tool_arg_from_str_any_available() {
    let arg = ToolArg::from_str("any-available").unwrap();
    assert!(matches!(arg, ToolArg::AnyAvailable));
}

#[test]
fn test_tool_arg_from_str_rejects_removed_gemini_cli() {
    let err = ToolArg::from_str("gemini-cli").unwrap_err();
    assert!(err.contains("no longer supported"), "{err}");
    assert!(err.contains("discontinued"), "{err}");
    assert!(err.contains("codex"), "{err}");
}

#[test]
fn test_tool_arg_from_str_specific_codex() {
    let arg = ToolArg::from_str("codex").unwrap();
    match arg {
        ToolArg::Specific(ToolName::Codex) => {}
        _ => panic!("Expected Specific(Codex)"),
    }
}

#[test]
fn test_tool_arg_from_str_unknown_becomes_alias() {
    let arg = ToolArg::from_str("invalid-tool").unwrap();
    assert!(matches!(arg, ToolArg::Alias(ref s) if s == "invalid-tool"));
}

#[test]
fn test_tool_arg_from_str_rejects_removed_gemini_alias() {
    let err = ToolArg::from_str("gemini").unwrap_err();
    assert!(
        err.contains("gemini-cli integration has been removed"),
        "{err}"
    );
}

#[test]
fn test_tool_arg_from_str_builtin_alias_claude() {
    let arg = ToolArg::from_str("claude").unwrap();
    assert!(matches!(arg, ToolArg::Specific(ToolName::ClaudeCode)));
}

#[test]
fn test_resolve_alias_with_config() {
    let mut aliases = HashMap::new();
    aliases.insert("router".to_string(), "codex".to_string());
    aliases.insert("cc".to_string(), "claude-code".to_string());

    let arg = ToolArg::from_str("router").unwrap();
    let resolved = arg.resolve_alias(&aliases).unwrap();
    assert!(matches!(resolved, ToolArg::Specific(ToolName::Codex)));

    let arg = ToolArg::from_str("cc").unwrap();
    let resolved = arg.resolve_alias(&aliases).unwrap();
    assert!(matches!(resolved, ToolArg::Specific(ToolName::ClaudeCode)));
}

#[test]
fn test_resolve_alias_rejects_removed_gemini_cli_target() {
    let mut aliases = HashMap::new();
    aliases.insert("gem".to_string(), "gemini-cli".to_string());
    let arg = ToolArg::from_str("gem").unwrap();
    let err = arg.resolve_alias(&aliases).unwrap_err();
    assert!(err.contains("no longer supported"), "{err}");
}

#[test]
fn test_resolve_alias_unknown_errors() {
    let aliases = HashMap::new();
    let arg = ToolArg::from_str("nonexistent").unwrap();
    let result = arg.resolve_alias(&aliases);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unknown tool 'nonexistent'"));
}

#[test]
fn test_resolve_alias_passthrough_for_non_alias() {
    let aliases = HashMap::new();
    let auto = ToolArg::Auto.resolve_alias(&aliases).unwrap();
    assert!(matches!(auto, ToolArg::Auto));
    let specific = ToolArg::Specific(ToolName::Codex)
        .resolve_alias(&aliases)
        .unwrap();
    assert!(matches!(specific, ToolArg::Specific(ToolName::Codex)));
}

#[test]
fn test_resolve_alias_chained_builtin() {
    // Config alias pointing to a built-in alias name
    let mut aliases = HashMap::new();
    aliases.insert("c".to_string(), "claude".to_string());
    let arg = ToolArg::from_str("c").unwrap();
    let resolved = arg.resolve_alias(&aliases).unwrap();
    assert!(matches!(resolved, ToolArg::Specific(ToolName::ClaudeCode)));
}

#[test]
fn test_tool_arg_into_strategy_auto() {
    let strategy = ToolArg::Auto.into_strategy();
    assert!(matches!(
        strategy,
        ToolSelectionStrategy::HeterogeneousPreferred
    ));
}

#[test]
fn test_tool_arg_into_strategy_any_available() {
    let strategy = ToolArg::AnyAvailable.into_strategy();
    assert!(matches!(strategy, ToolSelectionStrategy::AnyAvailable));
}

#[test]
fn test_tool_arg_into_strategy_specific() {
    let strategy = ToolArg::Specific(ToolName::Codex).into_strategy();
    match strategy {
        ToolSelectionStrategy::Explicit(ToolName::Codex) => {}
        _ => panic!("Expected Explicit(Codex)"),
    }
}

#[test]
fn test_tool_name_model_family() {
    assert_eq!(ToolName::ClaudeCode.model_family(), ModelFamily::Claude);
    assert_eq!(ToolName::GeminiCli.model_family(), ModelFamily::Gemini);
    assert_eq!(ToolName::Codex.model_family(), ModelFamily::OpenAI);
    assert_eq!(ToolName::Opencode.model_family(), ModelFamily::Other);
    assert_eq!(ToolName::Hermes.model_family(), ModelFamily::Other);
}

#[test]
fn test_tool_arg_display() {
    assert_eq!(ToolArg::Auto.to_string(), "auto");
    assert_eq!(ToolArg::AnyAvailable.to_string(), "any-available");
    assert_eq!(
        ToolArg::Specific(ToolName::GeminiCli).to_string(),
        "gemini-cli"
    );
}

#[test]
fn test_model_family_display() {
    assert_eq!(ModelFamily::Claude.to_string(), "Claude");
    assert_eq!(ModelFamily::Gemini.to_string(), "Gemini");
    assert_eq!(ModelFamily::OpenAI.to_string(), "OpenAI");
    assert_eq!(ModelFamily::Other.to_string(), "Other");
}

#[test]
fn test_tool_name_as_str() {
    assert_eq!(ToolName::GeminiCli.as_str(), "gemini-cli");
    assert_eq!(ToolName::Opencode.as_str(), "opencode");
    assert_eq!(ToolName::Codex.as_str(), "codex");
    assert_eq!(ToolName::ClaudeCode.as_str(), "claude-code");
    assert_eq!(ToolName::Hermes.as_str(), "hermes");
}

#[test]
fn test_tool_name_display() {
    assert_eq!(ToolName::GeminiCli.to_string(), "gemini-cli");
    assert_eq!(ToolName::Opencode.to_string(), "opencode");
    assert_eq!(ToolName::Codex.to_string(), "codex");
    assert_eq!(ToolName::ClaudeCode.to_string(), "claude-code");
    assert_eq!(ToolName::Hermes.to_string(), "hermes");
}

#[test]
fn test_tool_arg_from_str_specific_opencode() {
    let arg = ToolArg::from_str("opencode").unwrap();
    assert!(matches!(arg, ToolArg::Specific(ToolName::Opencode)));
}

#[test]
fn test_tool_arg_from_str_specific_claude_code() {
    let arg = ToolArg::from_str("claude-code").unwrap();
    assert!(matches!(arg, ToolArg::Specific(ToolName::ClaudeCode)));
}

#[test]
fn test_tool_arg_from_str_specific_hermes() {
    let arg = ToolArg::from_str("hermes").unwrap();
    assert!(matches!(arg, ToolArg::Specific(ToolName::Hermes)));
}

#[test]
fn test_tool_arg_display_fromstr_roundtrip() {
    let cases = [
        ToolArg::Auto,
        ToolArg::AnyAvailable,
        ToolArg::Specific(ToolName::Opencode),
        ToolArg::Specific(ToolName::Codex),
        ToolArg::Specific(ToolName::ClaudeCode),
        ToolArg::Specific(ToolName::OpenaiCompat),
        ToolArg::Specific(ToolName::Hermes),
        ToolArg::Specific(ToolName::AntigravityCli),
    ];
    for original in &cases {
        let s = original.to_string();
        let parsed = ToolArg::from_str(&s).unwrap();
        assert_eq!(parsed.to_string(), s);
    }
}

#[test]
fn test_tool_arg_into_strategy_all_specific() {
    let tools = [
        (ToolName::GeminiCli, "GeminiCli"),
        (ToolName::Opencode, "Opencode"),
        (ToolName::Codex, "Codex"),
        (ToolName::ClaudeCode, "ClaudeCode"),
        (ToolName::Hermes, "Hermes"),
    ];
    for (tool, label) in tools {
        let strategy = ToolArg::Specific(tool).into_strategy();
        match strategy {
            ToolSelectionStrategy::Explicit(t) => assert_eq!(t, tool, "Mismatch for {label}"),
            _ => panic!("Expected Explicit for {label}"),
        }
    }
}

#[test]
fn test_prompt_transport_capabilities() {
    assert_eq!(
        prompt_transport_capabilities(&ToolName::GeminiCli),
        &[PromptTransport::Argv, PromptTransport::Stdin]
    );
    assert_eq!(
        prompt_transport_capabilities(&ToolName::Codex),
        &[PromptTransport::Argv, PromptTransport::Stdin]
    );
    assert_eq!(
        prompt_transport_capabilities(&ToolName::ClaudeCode),
        &[PromptTransport::Argv, PromptTransport::Stdin]
    );
    assert_eq!(
        prompt_transport_capabilities(&ToolName::Hermes),
        &[PromptTransport::Argv, PromptTransport::Stdin]
    );
    assert_eq!(
        prompt_transport_capabilities(&ToolName::Opencode),
        &[PromptTransport::Argv]
    );
}

#[test]
fn test_tool_arg_from_str_empty_string_becomes_alias() {
    let arg = ToolArg::from_str("").unwrap();
    assert!(matches!(arg, ToolArg::Alias(ref s) if s.is_empty()));
}

#[test]
fn test_tool_arg_from_str_case_sensitive_becomes_alias() {
    // Tool names are case-sensitive: wrong case becomes Alias
    assert!(matches!(
        ToolArg::from_str("Auto").unwrap(),
        ToolArg::Alias(_)
    ));
    assert!(matches!(
        ToolArg::from_str("CODEX").unwrap(),
        ToolArg::Alias(_)
    ));
    assert!(matches!(
        ToolArg::from_str("Claude-Code").unwrap(),
        ToolArg::Alias(_)
    ));
}

#[test]
fn test_review_decision_from_str() {
    assert_eq!(
        ReviewDecision::from_str("pass").unwrap(),
        ReviewDecision::Pass
    );
    assert_eq!(
        ReviewDecision::from_str("CLEAN").unwrap(),
        ReviewDecision::Pass
    );
    assert_eq!(
        ReviewDecision::from_str("fail").unwrap(),
        ReviewDecision::Fail
    );
    assert_eq!(
        ReviewDecision::from_str("HAS_ISSUES").unwrap(),
        ReviewDecision::Fail
    );
    assert_eq!(
        ReviewDecision::from_str("skip").unwrap(),
        ReviewDecision::Skip
    );
    assert_eq!(
        ReviewDecision::from_str("uncertain").unwrap(),
        ReviewDecision::Uncertain
    );
    assert_eq!(
        ReviewDecision::from_str("UNAVAILABLE").unwrap(),
        ReviewDecision::Unavailable
    );
    assert!(ReviewDecision::from_str("invalid").is_err());
}

#[test]
fn test_review_decision_is_clean() {
    assert!(ReviewDecision::Pass.is_clean());
    assert!(ReviewDecision::Skip.is_clean());
    assert!(!ReviewDecision::Fail.is_clean());
    assert!(!ReviewDecision::Uncertain.is_clean());
    assert!(!ReviewDecision::Unavailable.is_clean());
}

#[test]
fn test_review_decision_display() {
    assert_eq!(ReviewDecision::Pass.to_string(), "pass");
    assert_eq!(ReviewDecision::Fail.to_string(), "fail");
    assert_eq!(ReviewDecision::Skip.to_string(), "skip");
    assert_eq!(ReviewDecision::Uncertain.to_string(), "uncertain");
    assert_eq!(ReviewDecision::Unavailable.to_string(), "unavailable");
}

#[test]
fn test_review_decision_serde_unavailable_roundtrip() {
    let encoded =
        serde_json::to_string(&ReviewDecision::Unavailable).expect("serialize unavailable");
    assert_eq!(encoded, "\"unavailable\"");

    let decoded: ReviewDecision =
        serde_json::from_str("\"unavailable\"").expect("deserialize unavailable");
    assert_eq!(decoded, ReviewDecision::Unavailable);

    let legacy: ReviewDecision =
        serde_json::from_str("\"UNAVAILABLE\"").expect("deserialize legacy unavailable");
    assert_eq!(legacy, ReviewDecision::Unavailable);
}
