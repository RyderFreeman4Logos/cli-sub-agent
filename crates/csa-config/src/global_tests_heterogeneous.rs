use super::*;
use std::collections::HashMap;

#[test]
fn test_select_heterogeneous_tool_claude_to_others() {
    let parent = ToolName::ClaudeCode;
    let available = vec![
        ToolName::ClaudeCode,
        ToolName::GeminiCli,
        ToolName::Codex,
        ToolName::Opencode,
    ];
    let result = select_heterogeneous_tool(&parent, &available);
    assert!(result.is_some());
    let tool = result.unwrap();
    assert_ne!(tool.model_family(), parent.model_family());
}

#[test]
fn test_select_heterogeneous_tool_gemini_to_others() {
    let parent = ToolName::GeminiCli;
    let available = vec![ToolName::GeminiCli, ToolName::Codex, ToolName::ClaudeCode];
    let result = select_heterogeneous_tool(&parent, &available);
    assert!(result.is_some());
    let tool = result.unwrap();
    assert_ne!(tool.model_family(), parent.model_family());
}

#[test]
fn test_select_heterogeneous_tool_none_when_all_same_family() {
    let parent = ToolName::ClaudeCode;
    let available = vec![ToolName::ClaudeCode]; // Only same family
    let result = select_heterogeneous_tool(&parent, &available);
    assert!(result.is_none());
}

#[test]
fn test_select_heterogeneous_tool_empty_available() {
    let parent = ToolName::ClaudeCode;
    let available = vec![];
    let result = select_heterogeneous_tool(&parent, &available);
    assert!(result.is_none());
}

#[test]
fn test_fallback_config_default() {
    let config = GlobalConfig::default();
    assert_eq!(config.fallback.cloud_review_exhausted, "ask-user");
}

#[test]
fn test_fallback_config_auto_local() {
    let toml_str = r#"
[fallback]
cloud_review_exhausted = "auto-local"
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.fallback.cloud_review_exhausted, "auto-local");
}

#[test]
fn test_fallback_config_missing_uses_default() {
    let toml_str = r#"
[defaults]
max_concurrent = 3
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.fallback.cloud_review_exhausted, "ask-user");
}

#[test]
fn test_all_known_tools() {
    let tools = all_known_tools();
    assert_eq!(tools.len(), 4);
    assert!(tools.contains(&ToolName::GeminiCli));
    assert!(tools.contains(&ToolName::Opencode));
    assert!(tools.contains(&ToolName::Codex));
    assert!(tools.contains(&ToolName::ClaudeCode));
}

#[test]
fn test_heterogeneous_counterpart_claude_to_codex() {
    assert_eq!(heterogeneous_counterpart("claude-code"), Some("codex"));
}

#[test]
fn test_heterogeneous_counterpart_codex_to_claude() {
    assert_eq!(heterogeneous_counterpart("codex"), Some("claude-code"));
}

#[test]
fn test_heterogeneous_counterpart_gemini_returns_none() {
    assert_eq!(heterogeneous_counterpart("gemini-cli"), None);
}

#[test]
fn test_heterogeneous_counterpart_opencode_returns_none() {
    assert_eq!(heterogeneous_counterpart("opencode"), None);
}

#[test]
fn test_heterogeneous_counterpart_unknown_returns_none() {
    assert_eq!(heterogeneous_counterpart("unknown-tool"), None);
    assert_eq!(heterogeneous_counterpart(""), None);
}

#[test]
fn test_all_tool_slots_includes_extra_config_tools() {
    let mut config = GlobalConfig::default();
    config.tools.insert(
        "custom-tool".to_string(),
        GlobalToolConfig {
            max_concurrent: Some(7),
            env: HashMap::new(),
            ..Default::default()
        },
    );

    let slots = config.all_tool_slots();
    // 4 static tools + 1 custom = 5
    assert_eq!(slots.len(), 5);
    let custom = slots.iter().find(|(t, _)| *t == "custom-tool").unwrap();
    assert_eq!(custom.1, 7);
}

#[test]
fn test_all_tool_slots_default_config_has_four_tools() {
    let config = GlobalConfig::default();
    let slots = config.all_tool_slots();
    assert_eq!(slots.len(), 4);

    let names: Vec<&str> = slots.iter().map(|(n, _)| *n).collect();
    assert!(names.contains(&"gemini-cli"));
    assert!(names.contains(&"opencode"));
    assert!(names.contains(&"codex"));
    assert!(names.contains(&"claude-code"));

    // All should have default concurrency
    for (_, count) in &slots {
        assert_eq!(*count, 3);
    }
}

#[test]
fn test_env_vars_multiple_keys() {
    let mut config = GlobalConfig::default();
    let mut env = HashMap::new();
    env.insert("API_KEY".to_string(), "key-1".to_string());
    env.insert("SECRET".to_string(), "secret-1".to_string());
    config.tools.insert(
        "codex".to_string(),
        GlobalToolConfig {
            max_concurrent: None,
            env,
            ..Default::default()
        },
    );

    let vars = config.env_vars("codex").unwrap();
    assert_eq!(vars.len(), 2);
    assert_eq!(vars.get("API_KEY").unwrap(), "key-1");
    assert_eq!(vars.get("SECRET").unwrap(), "secret-1");
}

#[test]
fn test_env_vars_nonexistent_tool_returns_none() {
    let config = GlobalConfig::default();
    assert!(config.env_vars("totally-unknown").is_none());
}

#[test]
fn test_max_concurrent_with_custom_default() {
    let mut config = GlobalConfig::default();
    config.defaults.max_concurrent = 10;

    // All tools without overrides should use the custom default
    assert_eq!(config.max_concurrent("gemini-cli"), 10);
    assert_eq!(config.max_concurrent("codex"), 10);
    assert_eq!(config.max_concurrent("unknown"), 10);

    // Tool-specific override still wins
    config.tools.insert(
        "codex".to_string(),
        GlobalToolConfig {
            max_concurrent: Some(2),
            env: HashMap::new(),
            ..Default::default()
        },
    );
    assert_eq!(config.max_concurrent("codex"), 2);
    assert_eq!(config.max_concurrent("gemini-cli"), 10); // still default
}

#[test]
fn test_max_concurrent_tool_with_none_uses_default() {
    let mut config = GlobalConfig::default();
    config.tools.insert(
        "codex".to_string(),
        GlobalToolConfig {
            max_concurrent: None, // explicitly None
            env: HashMap::new(),
            ..Default::default()
        },
    );
    assert_eq!(config.max_concurrent("codex"), 3); // falls back to default
}

#[test]
fn test_resolve_debate_tool_explicit_override() {
    let mut config = GlobalConfig::default();
    config.debate.tool = "opencode".to_string();
    // When explicitly set, should return the explicit value regardless of parent
    let tool = config.resolve_debate_tool(Some("anything")).unwrap();
    assert_eq!(tool, "opencode");
    let tool = config.resolve_debate_tool(None).unwrap();
    assert_eq!(tool, "opencode");
}

#[test]
fn test_resolve_review_tool_explicit_ignores_parent() {
    let mut config = GlobalConfig::default();
    config.review.tool = "gemini-cli".to_string();
    let tool = config.resolve_review_tool(None).unwrap();
    assert_eq!(tool, "gemini-cli");
}

#[test]
fn test_parse_toml_with_all_sections() {
    let toml_str = r#"
[defaults]
max_concurrent = 2

[tools.codex]
max_concurrent = 4
[tools.codex.env]
OPENAI_API_KEY = "sk-test"

[review]
tool = "codex"

[debate]
tool = "claude-code"

[fallback]
cloud_review_exhausted = "auto-local"

[todo]
show_command = "bat -l md"
diff_command = "delta"
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.defaults.max_concurrent, 2);
    assert_eq!(config.max_concurrent("codex"), 4);
    assert_eq!(config.max_concurrent("gemini-cli"), 2); // falls to default
    assert_eq!(config.review.tool, "codex");
    assert_eq!(config.debate.tool, "claude-code");
    assert_eq!(config.debate.timeout_seconds, 1800);
    assert_eq!(config.debate.thinking, None);
    assert_eq!(config.fallback.cloud_review_exhausted, "auto-local");
    assert_eq!(config.todo.show_command.as_deref(), Some("bat -l md"));
    assert_eq!(config.todo.diff_command.as_deref(), Some("delta"));
}

#[test]
fn test_parse_empty_toml() {
    let config: GlobalConfig = toml::from_str("").unwrap();
    assert_eq!(config.defaults.max_concurrent, 3);
    assert!(config.defaults.tool.is_none());
    assert!(config.tools.is_empty());
    assert_eq!(config.review.tool, "auto");
    assert_eq!(config.debate.tool, "auto");
    assert_eq!(config.debate.timeout_seconds, 1800);
    assert_eq!(config.debate.thinking, None);
    assert_eq!(config.fallback.cloud_review_exhausted, "ask-user");
}

#[test]
fn test_defaults_config_deserialization_with_tool() {
    let toml_str = r#"
[defaults]
max_concurrent = 4
tool = "codex"
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.defaults.max_concurrent, 4);
    assert_eq!(config.defaults.tool.as_deref(), Some("codex"));
}

#[test]
fn test_defaults_config_deserialization_without_tool() {
    let toml_str = r#"
[defaults]
max_concurrent = 4
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.defaults.max_concurrent, 4);
    assert!(config.defaults.tool.is_none());
}

#[test]
fn test_resolve_auto_tool_error_includes_section_name() {
    let config = GlobalConfig::default();
    let err = config.resolve_review_tool(Some("gemini-cli")).unwrap_err();
    assert!(err.to_string().contains("review"));

    let err = config.resolve_debate_tool(Some("gemini-cli")).unwrap_err();
    assert!(err.to_string().contains("debate"));
}

#[test]
fn test_todo_display_config_default() {
    let config = GlobalConfig::default();
    assert!(config.todo.show_command.is_none());
    assert!(config.todo.diff_command.is_none());
}

#[test]
fn test_state_base_dir_returns_ok() {
    let dir = GlobalConfig::state_base_dir();
    assert!(dir.is_ok());
    let path = dir.unwrap();
    let path_str = path.to_string_lossy();
    assert!(
        path_str.contains("cli-sub-agent") || path_str.contains("csa"),
        "unexpected state dir path: {path_str}"
    );
}

// ── Task 5: ExecutionConfig in GlobalConfig ─────────────────────────

#[test]
fn test_global_execution_config_default() {
    let config = GlobalConfig::default();
    assert!(config.execution.is_default());
    assert_eq!(config.execution.min_timeout_seconds, 1800);
}

#[test]
fn test_global_execution_config_parses_from_toml() {
    let toml_str = r#"
[execution]
min_timeout_seconds = 2400
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.execution.min_timeout_seconds, 2400);
    assert!(!config.execution.is_default());
}

#[test]
fn test_global_execution_config_empty_toml_uses_default() {
    let config: GlobalConfig = toml::from_str("").unwrap();
    assert_eq!(config.execution.min_timeout_seconds, 1800);
    assert!(config.execution.is_default());
}

#[test]
fn test_global_execution_config_coexists_with_other_sections() {
    let toml_str = r#"
[defaults]
max_concurrent = 5

[execution]
min_timeout_seconds = 3600

[review]
tool = "codex"
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.defaults.max_concurrent, 5);
    assert_eq!(config.execution.min_timeout_seconds, 3600);
    assert_eq!(config.review.tool, "codex");
}

#[test]
fn test_global_default_template_mentions_execution() {
    let template = GlobalConfig::default_template();
    assert!(
        template.contains("min_timeout_seconds"),
        "Default template should mention min_timeout_seconds"
    );
}
