use super::*;
use csa_core::gemini::{
    API_KEY_ENV, API_KEY_FALLBACK_ENV_KEY, AUTH_MODE_API_KEY, AUTH_MODE_ENV_KEY, AUTH_MODE_OAUTH,
    NO_FLASH_FALLBACK_ENV_KEY,
};
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
fn test_build_execution_env_adds_gemini_fallback_and_oauth_mode() {
    let mut config = GlobalConfig::default();
    config.tools.insert(
        "gemini-cli".to_string(),
        GlobalToolConfig {
            api_key: Some("fallback-key".to_string()),
            ..Default::default()
        },
    );

    let env = config
        .build_execution_env("gemini-cli", ExecutionEnvOptions::default())
        .unwrap();
    assert_eq!(
        env.get(API_KEY_FALLBACK_ENV_KEY).map(String::as_str),
        Some("fallback-key")
    );
    assert_eq!(
        env.get(AUTH_MODE_ENV_KEY).map(String::as_str),
        Some(AUTH_MODE_OAUTH)
    );
}

#[test]
fn test_build_execution_env_detects_api_key_mode_and_no_flash() {
    let mut config = GlobalConfig::default();
    let mut env = HashMap::new();
    env.insert(API_KEY_ENV.to_string(), "configured-key".to_string());
    config.tools.insert(
        "gemini-cli".to_string(),
        GlobalToolConfig {
            env,
            ..Default::default()
        },
    );

    let env = config
        .build_execution_env("gemini-cli", ExecutionEnvOptions::with_no_flash_fallback())
        .unwrap();
    assert_eq!(
        env.get(AUTH_MODE_ENV_KEY).map(String::as_str),
        Some(AUTH_MODE_API_KEY)
    );
    assert_eq!(
        env.get(NO_FLASH_FALLBACK_ENV_KEY).map(String::as_str),
        Some("1")
    );
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
    assert!(config.review.tool.is_auto());
    assert_eq!(config.review.gate_mode, GateMode::Monitor);
}

#[test]
fn test_gate_mode_default_is_monitor() {
    assert_eq!(GateMode::default(), GateMode::Monitor);
}

#[test]
fn test_debate_config_default() {
    let config = GlobalConfig::default();
    assert!(config.debate.tool.is_auto());
    assert_eq!(config.debate.timeout_seconds, 1800);
    assert_eq!(config.debate.thinking, None);
    assert!(config.debate.same_model_fallback);
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
    config.review.tool = ToolSelection::Single("opencode".to_string());
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
    assert_eq!(
        config.review.tool,
        ToolSelection::Single("codex".to_string())
    );
    assert_eq!(config.review.gate_mode, GateMode::Monitor);
}

#[test]
fn test_parse_review_config_with_gate_mode_critical_only() {
    let toml_str = r#"
[review]
tool = "codex"
gate_mode = "critical_only"
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.review.gate_mode, GateMode::CriticalOnly);
}

#[test]
fn test_parse_review_config_with_gate_mode_full() {
    let toml_str = r#"
[review]
tool = "codex"
gate_mode = "full"
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.review.gate_mode, GateMode::Full);
}

#[test]
fn test_gate_mode_serde_roundtrip_all_variants() {
    for gate_mode in [GateMode::Monitor, GateMode::CriticalOnly, GateMode::Full] {
        let review = ReviewConfig {
            tool: ToolSelection::Single("codex".to_string()),
            gate_mode: gate_mode.clone(),
            tier: None,
            model: None,
            thinking: None,
            gate_command: None,
            gate_commands: vec![],
            gate_timeout_secs: ReviewConfig::default_gate_timeout(),
            readonly_sandbox: None,
        };
        let toml = toml::to_string(&review).unwrap();
        let parsed: ReviewConfig = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.tool, review.tool);
        assert_eq!(parsed.gate_mode, review.gate_mode);
        assert_eq!(parsed.gate_command, review.gate_command);
        assert_eq!(parsed.gate_timeout_secs, review.gate_timeout_secs);
    }
}

#[test]
fn test_review_config_gate_command_parses() {
    let toml_str = r#"
[review]
tool = "auto"
gate_command = "just pre-commit"
gate_timeout_secs = 600
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(
        config.review.gate_command.as_deref(),
        Some("just pre-commit")
    );
    assert_eq!(config.review.gate_timeout_secs, 600);
}

#[test]
fn test_review_config_gate_fields_default() {
    let config = ReviewConfig::default();
    assert!(config.gate_command.is_none());
    assert_eq!(config.gate_timeout_secs, 300);
}

#[test]
fn test_review_config_is_default() {
    let config = ReviewConfig::default();
    assert!(config.is_default());
}

#[test]
fn test_review_config_is_not_default_with_gate_command() {
    let config = ReviewConfig {
        gate_command: Some("make lint".to_string()),
        ..Default::default()
    };
    assert!(!config.is_default());
}

#[test]
fn test_review_config_is_not_default_with_gate_timeout() {
    let config = ReviewConfig {
        gate_timeout_secs: 600,
        ..Default::default()
    };
    assert!(!config.is_default());
}

#[test]
fn test_review_config_gate_timeout_default_skipped_in_serialization() {
    let config = ReviewConfig::default();
    let toml_str = toml::to_string(&config).unwrap();
    // Default gate_timeout_secs (300) should be skipped via skip_serializing_if
    assert!(
        !toml_str.contains("gate_timeout_secs"),
        "Default gate_timeout_secs should be omitted from TOML output"
    );
    // gate_command=None should also be omitted
    assert!(
        !toml_str.contains("gate_command"),
        "None gate_command should be omitted from TOML output"
    );
}

#[test]
fn test_review_config_gate_timeout_non_default_serialized() {
    let config = ReviewConfig {
        gate_timeout_secs: 600,
        ..Default::default()
    };
    let toml_str = toml::to_string(&config).unwrap();
    assert!(
        toml_str.contains("gate_timeout_secs = 600"),
        "Non-default gate_timeout_secs should appear in TOML output"
    );
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
    assert_eq!(
        config.debate.tool,
        ToolSelection::Single("codex".to_string())
    );
    assert_eq!(config.debate.timeout_seconds, 2400);
    assert_eq!(config.debate.thinking.as_deref(), Some("high"));
    // same_model_fallback defaults to true when not specified
    assert!(config.debate.same_model_fallback);
}

#[test]
fn test_parse_review_config_with_model() {
    let toml_str = r#"
[review]
tool = "gemini-cli"
model = "gemini-3.1-pro-preview"
thinking = "xhigh"
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(
        config.review.model.as_deref(),
        Some("gemini-3.1-pro-preview")
    );
    assert!(!config.review.is_default());
}

#[test]
fn test_review_config_model_none_is_default() {
    let config = ReviewConfig::default();
    assert!(config.model.is_none());
    assert!(config.is_default());
}

#[test]
fn test_review_config_model_skipped_when_none() {
    let config = ReviewConfig::default();
    let toml_str = toml::to_string(&config).unwrap();
    assert!(
        !toml_str.contains("model"),
        "None model should be omitted from TOML output: {toml_str}"
    );
}

#[test]
fn test_parse_debate_config_with_model() {
    let toml_str = r#"
[debate]
tool = "gemini-cli"
model = "gemini-3.1-pro-preview"
thinking = "xhigh"
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(
        config.debate.model.as_deref(),
        Some("gemini-3.1-pro-preview")
    );
    assert!(!config.debate.is_default());
}

#[test]
fn test_debate_config_model_none_is_default() {
    let config = DebateConfig::default();
    assert!(config.model.is_none());
    assert!(config.is_default());
}

#[test]
fn test_parse_debate_config_same_model_fallback_disabled() {
    let toml_str = r#"
[debate]
tool = "auto"
same_model_fallback = false
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert!(!config.debate.same_model_fallback);
}

// --- ACP config regression tests (issue #417) ---

#[test]
fn global_config_acp_init_timeout_from_toml() {
    let toml_str = r#"
[acp]
init_timeout_seconds = 180
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.acp.init_timeout_seconds, 180);
}

#[test]
fn global_config_acp_default_is_120() {
    let config = GlobalConfig::default();
    assert_eq!(config.acp.init_timeout_seconds, 120);
}

#[test]
fn default_template_contains_acp_section() {
    let template = GlobalConfig::default_template();
    assert!(
        template.contains("init_timeout_seconds"),
        "template should mention init_timeout_seconds"
    );
}

// --- readonly_sandbox config tests ---

#[test]
fn test_parse_review_readonly_sandbox_true() {
    let toml_str = r#"
[review]
readonly_sandbox = true
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.review.readonly_sandbox, Some(true));
    assert!(!config.review.is_default());
}

#[test]
fn test_parse_debate_readonly_sandbox_false() {
    let toml_str = r#"
[debate]
readonly_sandbox = false
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.debate.readonly_sandbox, Some(false));
    assert!(!config.debate.is_default());
}

#[test]
fn test_readonly_sandbox_default_is_none() {
    let config = GlobalConfig::default();
    assert_eq!(config.review.readonly_sandbox, None);
    assert_eq!(config.debate.readonly_sandbox, None);
}

#[test]
fn test_readonly_sandbox_omitted_from_toml_when_none() {
    let config = ReviewConfig::default();
    let toml_str = toml::to_string(&config).unwrap();
    assert!(
        !toml_str.contains("readonly_sandbox"),
        "None readonly_sandbox should be omitted: {toml_str}"
    );
}

#[test]
fn test_readonly_sandbox_serialized_when_set() {
    let config = ReviewConfig {
        readonly_sandbox: Some(true),
        ..Default::default()
    };
    let toml_str = toml::to_string(&config).unwrap();
    assert!(
        toml_str.contains("readonly_sandbox = true"),
        "Set readonly_sandbox should appear in TOML: {toml_str}"
    );
}
