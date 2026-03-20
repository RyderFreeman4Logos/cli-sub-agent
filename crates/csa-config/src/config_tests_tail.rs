use super::*;

#[test]
fn test_enforce_tool_enabled_enabled_tool_returns_ok() {
    let mut tools = HashMap::new();
    tools.insert("codex".to_string(), ToolConfig::default());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    assert!(config.enforce_tool_enabled("codex", false).is_ok());
}

#[test]
fn test_enforce_tool_enabled_unconfigured_tool_returns_ok() {
    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    assert!(config.enforce_tool_enabled("codex", false).is_ok());
}

#[test]
fn test_enforce_tool_enabled_force_override_bypasses_disabled() {
    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: false,
            ..Default::default()
        },
    );

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    assert!(config.enforce_tool_enabled("codex", true).is_ok());
}

// ── SessionConfig tests ──────────────────────────────────────────

#[test]
fn test_session_config_default_has_structured_output_enabled() {
    let cfg = SessionConfig::default();
    assert!(cfg.structured_output);
    assert_eq!(cfg.resolved_spool_max_mb(), 32);
    assert!(cfg.resolved_spool_keep_rotated());
}

#[test]
fn test_session_config_is_default_reflects_structured_output() {
    let mut cfg = SessionConfig::default();
    assert!(cfg.is_default());

    cfg.structured_output = false;
    assert!(!cfg.is_default());
}

#[test]
fn test_session_config_deserializes_structured_output() {
    let toml_str = r#"
transcript_enabled = false
transcript_redaction = true
structured_output = false
"#;
    let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
    assert!(!cfg.structured_output);
}

#[test]
fn test_session_config_defaults_structured_output_when_missing() {
    let toml_str = r#"
transcript_enabled = false
"#;
    let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
    assert!(cfg.structured_output);
}

#[test]
fn test_session_config_default_does_not_require_commit_on_mutation() {
    let cfg = SessionConfig::default();
    assert!(!cfg.require_commit_on_mutation);
}

#[test]
fn test_session_config_deserializes_require_commit_on_mutation() {
    let toml_str = r#"
transcript_enabled = false
require_commit_on_mutation = true
"#;
    let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
    assert!(cfg.require_commit_on_mutation);
}

#[test]
fn test_session_config_is_default_reflects_require_commit_on_mutation() {
    let cfg = SessionConfig {
        require_commit_on_mutation: true,
        ..Default::default()
    };
    assert!(!cfg.is_default());
}

#[test]
fn test_session_config_deserializes_spool_settings() {
    let toml_str = r#"
spool_max_mb = 64
spool_keep_rotated = false
"#;
    let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.spool_max_mb, Some(64));
    assert_eq!(cfg.spool_keep_rotated, Some(false));
    assert_eq!(cfg.resolved_spool_max_mb(), 64);
    assert!(!cfg.resolved_spool_keep_rotated());
}

#[test]
fn test_session_config_is_default_reflects_spool_overrides() {
    let cfg = SessionConfig {
        spool_max_mb: Some(64),
        ..Default::default()
    };
    assert!(!cfg.is_default());

    let cfg = SessionConfig {
        spool_keep_rotated: Some(false),
        ..Default::default()
    };
    assert!(!cfg.is_default());
}

// ---------------------------------------------------------------------------
// ResourcesConfig: initial_response_timeout_seconds
// ---------------------------------------------------------------------------

#[test]
fn test_resources_config_default_has_initial_response_timeout_120() {
    let cfg = ResourcesConfig::default();
    assert_eq!(cfg.initial_response_timeout_seconds, Some(120));
}

#[test]
fn test_resources_config_is_default_with_default_initial_response_timeout() {
    let cfg = ResourcesConfig::default();
    assert!(cfg.is_default());
}

#[test]
fn test_resources_config_is_default_false_with_custom_initial_response_timeout() {
    let mut cfg = ResourcesConfig::default();
    cfg.initial_response_timeout_seconds = Some(60);
    assert!(!cfg.is_default());
}

#[test]
fn test_resources_config_deser_initial_response_timeout_custom() {
    let toml_str = r#"
initial_response_timeout_seconds = 60
"#;
    let cfg: ResourcesConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.initial_response_timeout_seconds, Some(60));
}

#[test]
fn test_resources_config_deser_initial_response_timeout_zero_disabled() {
    let toml_str = r#"
initial_response_timeout_seconds = 0
"#;
    let cfg: ResourcesConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.initial_response_timeout_seconds, Some(0));
}

#[test]
fn test_resources_config_deser_initial_response_timeout_omitted_defaults_to_120() {
    let toml_str = r#"
idle_timeout_seconds = 300
"#;
    let cfg: ResourcesConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.initial_response_timeout_seconds, Some(120));
}
