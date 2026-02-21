use super::*;
use std::collections::HashMap;

#[test]
fn test_default_config() {
    let config = GlobalConfig::default();
    assert_eq!(config.defaults.max_concurrent, 3);
    assert!(config.tools.is_empty());
}

#[test]
fn test_max_concurrent_default() {
    let config = GlobalConfig::default();
    assert_eq!(config.max_concurrent("gemini-cli"), 3);
    assert_eq!(config.max_concurrent("codex"), 3);
}

#[test]
fn test_max_concurrent_tool_override() {
    let mut config = GlobalConfig::default();
    config.tools.insert(
        "gemini-cli".to_string(),
        GlobalToolConfig {
            max_concurrent: Some(5),
            env: HashMap::new(),
            ..Default::default()
        },
    );
    assert_eq!(config.max_concurrent("gemini-cli"), 5);
    assert_eq!(config.max_concurrent("codex"), 3); // falls back to default
}

#[test]
fn test_env_vars() {
    let mut config = GlobalConfig::default();
    let mut env = HashMap::new();
    env.insert("GEMINI_API_KEY".to_string(), "test-key".to_string());
    config.tools.insert(
        "gemini-cli".to_string(),
        GlobalToolConfig {
            max_concurrent: None,
            env,
            ..Default::default()
        },
    );

    let vars = config.env_vars("gemini-cli").unwrap();
    assert_eq!(vars.get("GEMINI_API_KEY").unwrap(), "test-key");
    assert!(config.env_vars("codex").is_none());
}

#[test]
fn test_env_vars_empty_returns_none() {
    let mut config = GlobalConfig::default();
    config.tools.insert(
        "codex".to_string(),
        GlobalToolConfig {
            max_concurrent: Some(2),
            env: HashMap::new(),
            ..Default::default()
        },
    );
    assert!(config.env_vars("codex").is_none());
}

#[test]
fn test_parse_toml() {
    let toml_str = r#"
[defaults]
max_concurrent = 5

[tools.gemini-cli]
max_concurrent = 10

[tools.gemini-cli.env]
GEMINI_API_KEY = "test-key-123"

[tools.claude-code]
max_concurrent = 1
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.defaults.max_concurrent, 5);
    assert_eq!(config.max_concurrent("gemini-cli"), 10);
    assert_eq!(config.max_concurrent("claude-code"), 1);
    assert_eq!(config.max_concurrent("codex"), 5); // default

    let env = config.env_vars("gemini-cli").unwrap();
    assert_eq!(env.get("GEMINI_API_KEY").unwrap(), "test-key-123");
}

#[test]
fn test_load_missing_file() {
    // GlobalConfig::load() should return default when file doesn't exist
    // We can't easily test this without mocking config_path, but we can
    // verify the default is sane
    let config = GlobalConfig::default();
    assert_eq!(config.max_concurrent("any-tool"), 3);
}

#[test]
fn test_all_tool_slots() {
    let mut config = GlobalConfig::default();
    config.tools.insert(
        "gemini-cli".to_string(),
        GlobalToolConfig {
            max_concurrent: Some(5),
            env: HashMap::new(),
            ..Default::default()
        },
    );

    let slots = config.all_tool_slots();
    assert!(slots.len() >= 4);

    // gemini-cli should have override
    let gemini = slots.iter().find(|(t, _)| *t == "gemini-cli").unwrap();
    assert_eq!(gemini.1, 5);

    // codex should have default
    let codex = slots.iter().find(|(t, _)| *t == "codex").unwrap();
    assert_eq!(codex.1, 3);
}

#[test]
fn test_default_template_is_valid_comment_only() {
    let template = GlobalConfig::default_template();
    // The template should contain helpful comments
    assert!(template.contains("[defaults]"));
    assert!(template.contains("max_concurrent"));
    assert!(template.contains("# tool = \"codex\""));
}

#[test]
fn test_review_config_default() {
    let config = GlobalConfig::default();
    assert_eq!(config.review.tool, "auto");
}

#[test]
fn test_debate_config_default() {
    let config = GlobalConfig::default();
    assert_eq!(config.debate.tool, "auto");
    assert_eq!(config.debate.timeout_seconds, 1800);
    assert_eq!(config.debate.thinking, None);
}

#[test]
fn test_resolve_review_tool_auto_claude_code_parent() {
    let config = GlobalConfig::default();
    let tool = config.resolve_review_tool(Some("claude-code")).unwrap();
    assert_eq!(tool, "codex");
}

#[test]
fn test_resolve_review_tool_auto_codex_parent() {
    let config = GlobalConfig::default();
    let tool = config.resolve_review_tool(Some("codex")).unwrap();
    assert_eq!(tool, "claude-code");
}

#[test]
fn test_resolve_review_tool_auto_unknown_parent() {
    let config = GlobalConfig::default();
    let result = config.resolve_review_tool(Some("opencode"));
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("opencode"));
}

#[test]
fn test_resolve_review_tool_auto_no_parent() {
    let config = GlobalConfig::default();
    let result = config.resolve_review_tool(None);
    assert!(result.is_err());
}

#[test]
fn test_resolve_review_tool_explicit() {
    let mut config = GlobalConfig::default();
    config.review.tool = "opencode".to_string();
    let tool = config.resolve_review_tool(Some("anything")).unwrap();
    assert_eq!(tool, "opencode");
}

#[test]
fn test_parse_review_config() {
    let toml_str = r#"
[review]
tool = "codex"
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.review.tool, "codex");
}

#[test]
fn test_resolve_debate_tool_auto_claude_code_parent() {
    let config = GlobalConfig::default();
    let tool = config.resolve_debate_tool(Some("claude-code")).unwrap();
    assert_eq!(tool, "codex");
}

#[test]
fn test_resolve_debate_tool_auto_codex_parent() {
    let config = GlobalConfig::default();
    let tool = config.resolve_debate_tool(Some("codex")).unwrap();
    assert_eq!(tool, "claude-code");
}

#[test]
fn test_resolve_debate_tool_auto_unknown_parent() {
    let config = GlobalConfig::default();
    let result = config.resolve_debate_tool(Some("opencode"));
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("opencode"));
}

#[test]
fn test_resolve_debate_tool_auto_no_parent() {
    let config = GlobalConfig::default();
    let result = config.resolve_debate_tool(None);
    assert!(result.is_err());
}

#[test]
fn test_parse_debate_config() {
    let toml_str = r#"
[debate]
tool = "codex"
timeout_seconds = 2400
thinking = "high"
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.debate.tool, "codex");
    assert_eq!(config.debate.timeout_seconds, 2400);
    assert_eq!(config.debate.thinking.as_deref(), Some("high"));
}

#[test]
fn test_slots_dir() {
    // Should not fail on supported platforms
    let dir = GlobalConfig::slots_dir();
    assert!(dir.is_ok());
    let path = dir.unwrap();
    assert!(path.to_string_lossy().contains("slots"));
}

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
    assert!(path.to_string_lossy().contains("csa"));
}

// --- Tool Priority Tests ---

#[test]
fn sort_empty_priority_preserves_order() {
    let tools = vec![
        ToolName::GeminiCli,
        ToolName::Opencode,
        ToolName::Codex,
        ToolName::ClaudeCode,
    ];
    let result = sort_tools_by_priority(&tools, &[]);
    assert_eq!(
        result, tools,
        "empty priority should preserve original order"
    );
}

#[test]
fn sort_full_reorder() {
    let tools = vec![
        ToolName::GeminiCli,
        ToolName::Opencode,
        ToolName::Codex,
        ToolName::ClaudeCode,
    ];
    let priority = vec![
        "claude-code".to_string(),
        "codex".to_string(),
        "gemini-cli".to_string(),
        "opencode".to_string(),
    ];
    let result = sort_tools_by_priority(&tools, &priority);
    assert_eq!(
        result,
        vec![
            ToolName::ClaudeCode,
            ToolName::Codex,
            ToolName::GeminiCli,
            ToolName::Opencode,
        ]
    );
}

#[test]
fn sort_partial_priority() {
    let tools = vec![
        ToolName::GeminiCli,
        ToolName::Opencode,
        ToolName::Codex,
        ToolName::ClaudeCode,
    ];
    // Only list claude-code; others retain relative order after it.
    let priority = vec!["claude-code".to_string()];
    let result = sort_tools_by_priority(&tools, &priority);
    assert_eq!(
        result[0],
        ToolName::ClaudeCode,
        "listed tool should come first"
    );
    // Unlisted tools retain their relative order.
    let unlisted: Vec<_> = result.iter().skip(1).copied().collect();
    assert_eq!(
        unlisted,
        vec![ToolName::GeminiCli, ToolName::Opencode, ToolName::Codex],
        "unlisted tools should retain relative order"
    );
}

#[test]
fn sort_by_priority_method_uses_preferences() {
    let mut config = GlobalConfig::default();
    config.preferences.tool_priority = vec!["codex".to_string(), "claude-code".to_string()];

    let tools = all_known_tools();
    let result = config.sort_by_priority(tools);
    assert_eq!(result[0], ToolName::Codex);
    assert_eq!(result[1], ToolName::ClaudeCode);
}

#[test]
fn preferences_deserialize() {
    let toml_str = r#"
[preferences]
tool_priority = ["claude-code", "codex"]
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.preferences.tool_priority.len(), 2);
    assert_eq!(config.preferences.tool_priority[0], "claude-code");
    assert_eq!(config.preferences.tool_priority[1], "codex");
}

#[test]
fn preferences_default_empty() {
    let config = GlobalConfig::default();
    assert!(
        config.preferences.tool_priority.is_empty(),
        "default tool_priority should be empty"
    );
}

#[test]
fn default_template_contains_preferences_section() {
    let template = GlobalConfig::default_template();
    assert!(
        template.contains("[preferences]"),
        "template should contain commented [preferences] section"
    );
    assert!(
        template.contains("tool_priority"),
        "template should contain tool_priority key"
    );
}

#[test]
fn effective_tool_priority_uses_global_when_no_project() {
    let mut gc = GlobalConfig::default();
    gc.preferences.tool_priority = vec!["codex".into(), "claude-code".into()];
    let result = effective_tool_priority(None, &gc);
    assert_eq!(result, &["codex", "claude-code"]);
}

/// Helper: create a minimal ProjectConfig with given preferences.
fn project_config_with_preferences(prefs: Option<PreferencesConfig>) -> crate::ProjectConfig {
    use crate::config::{CURRENT_SCHEMA_VERSION, ProjectMeta, ResourcesConfig};
    use chrono::Utc;
    crate::ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: prefs,
        session: Default::default(),
    }
}

#[test]
fn effective_tool_priority_uses_project_override() {
    let mut gc = GlobalConfig::default();
    gc.preferences.tool_priority = vec!["codex".into(), "claude-code".into()];
    let pc = project_config_with_preferences(Some(PreferencesConfig {
        tool_priority: vec!["gemini-cli".into(), "opencode".into()],
    }));
    let result = effective_tool_priority(Some(&pc), &gc);
    assert_eq!(result, &["gemini-cli", "opencode"]);
}

#[test]
fn effective_tool_priority_falls_back_when_project_empty() {
    let mut gc = GlobalConfig::default();
    gc.preferences.tool_priority = vec!["codex".into()];
    let pc = project_config_with_preferences(Some(PreferencesConfig {
        tool_priority: vec![],
    }));
    let result = effective_tool_priority(Some(&pc), &gc);
    assert_eq!(result, &["codex"]);
}

#[test]
fn sort_tools_by_effective_priority_project_override() {
    let gc = GlobalConfig::default(); // empty global priority
    let pc = project_config_with_preferences(Some(PreferencesConfig {
        tool_priority: vec!["opencode".into(), "codex".into()],
    }));
    let tools = all_known_tools();
    let sorted = sort_tools_by_effective_priority(tools, Some(&pc), &gc);
    assert_eq!(sorted[0], ToolName::Opencode);
    assert_eq!(sorted[1], ToolName::Codex);
}
