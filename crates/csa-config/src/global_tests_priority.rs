use super::*;
use std::collections::HashMap;

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
primary_writer_spec = "codex/openai/gpt-5.4/high"
tool_priority = ["claude-code", "codex"]
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(
        config.preferences.primary_writer_spec.as_deref(),
        Some("codex/openai/gpt-5.4/high")
    );
    assert_eq!(config.preferences.tool_priority.len(), 2);
    assert_eq!(config.preferences.tool_priority[0], "claude-code");
    assert_eq!(config.preferences.tool_priority[1], "codex");
}

#[test]
fn preferences_default_empty() {
    let config = GlobalConfig::default();
    assert!(
        config.preferences.primary_writer_spec.is_none(),
        "default primary_writer_spec should be unset"
    );
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
    assert!(
        template.contains("primary_writer_spec"),
        "template should contain primary_writer_spec key"
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
        tool_aliases: HashMap::new(),
        preferences: prefs,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

#[test]
fn effective_tool_priority_uses_project_override() {
    let mut gc = GlobalConfig::default();
    gc.preferences.tool_priority = vec!["codex".into(), "claude-code".into()];
    let pc = project_config_with_preferences(Some(PreferencesConfig {
        tool_priority: vec!["gemini-cli".into(), "opencode".into()],
        ..Default::default()
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
        ..Default::default()
    }));
    let result = effective_tool_priority(Some(&pc), &gc);
    assert_eq!(result, &["codex"]);
}

#[test]
fn sort_tools_by_effective_priority_project_override() {
    let gc = GlobalConfig::default(); // empty global priority
    let pc = project_config_with_preferences(Some(PreferencesConfig {
        tool_priority: vec!["opencode".into(), "codex".into()],
        ..Default::default()
    }));
    let tools = all_known_tools();
    let sorted = sort_tools_by_effective_priority(tools, Some(&pc), &gc);
    assert_eq!(sorted[0], ToolName::Opencode);
    assert_eq!(sorted[1], ToolName::Codex);
}

#[test]
fn effective_primary_writer_spec_uses_global_when_no_project() {
    let mut gc = GlobalConfig::default();
    gc.preferences.primary_writer_spec = Some("codex/openai/gpt-5.4/high".into());

    let result = effective_primary_writer_spec(None, &gc);

    assert_eq!(result, Some("codex/openai/gpt-5.4/high"));
}

#[test]
fn effective_primary_writer_spec_uses_project_override() {
    let mut gc = GlobalConfig::default();
    gc.preferences.primary_writer_spec = Some("codex/openai/gpt-5.4/high".into());
    let pc = project_config_with_preferences(Some(PreferencesConfig {
        primary_writer_spec: Some("claude-code/anthropic/default/xhigh".into()),
        ..Default::default()
    }));

    let result = effective_primary_writer_spec(Some(&pc), &gc);

    assert_eq!(result, Some("claude-code/anthropic/default/xhigh"));
}

#[test]
fn effective_primary_writer_spec_falls_back_when_project_empty() {
    let mut gc = GlobalConfig::default();
    gc.preferences.primary_writer_spec = Some("codex/openai/gpt-5.4/high".into());
    let pc = project_config_with_preferences(Some(PreferencesConfig {
        primary_writer_spec: None,
        ..Default::default()
    }));

    let result = effective_primary_writer_spec(Some(&pc), &gc);

    assert_eq!(result, Some("codex/openai/gpt-5.4/high"));
}

#[test]
fn primary_writer_spec_can_be_set_on_preferences() {
    let mut config = GlobalConfig::default();

    config.preferences.primary_writer_spec = Some("openai-compat/openai/gpt-5/high".into());

    assert_eq!(
        config.preferences.primary_writer_spec.as_deref(),
        Some("openai-compat/openai/gpt-5/high")
    );
}
