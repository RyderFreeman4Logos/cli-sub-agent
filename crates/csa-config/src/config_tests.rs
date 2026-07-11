use super::*;
use tempfile::tempdir;

#[test]
fn test_save_and_load_roundtrip() {
    let dir = tempdir().unwrap();

    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: true,
            default_model: Some("gpt-5.4".to_string()),
            default_thinking: Some("xhigh".to_string()),
            ..Default::default()
        },
    );

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test-project".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    config.save(dir.path()).unwrap();

    // Use load_with_paths to avoid merging with real global config
    // (which may have gemini-cli disabled, overriding the test value).
    let project_path = dir.path().join(".csa").join("config.toml");
    let loaded = ProjectConfig::load_with_paths(None, &project_path).unwrap();
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();

    assert_eq!(loaded.project.name, "test-project");
    assert_eq!(loaded.project.max_recursion_depth, 5);
    assert!(loaded.tools.contains_key("codex"));
    let codex = loaded.tools.get("codex").unwrap();
    assert!(codex.enabled);
    assert_eq!(codex.default_model.as_deref(), Some("gpt-5.4"));
    assert_eq!(codex.default_thinking.as_deref(), Some("xhigh"));
}

#[test]
fn test_tool_state_dirs_roundtrip() {
    let dir = tempdir().unwrap();
    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "state-dir-test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tool_state_dirs: HashMap::from([
            ("codex".to_string(), PathBuf::from("~/.codex")),
            ("claude".to_string(), PathBuf::from("~/.claude")),
        ]),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    config.save(dir.path()).unwrap();
    let project_path = dir.path().join(".csa").join("config.toml");
    let saved = std::fs::read_to_string(&project_path).unwrap();
    assert!(saved.contains("[tool_state_dirs]"));
    assert!(saved.contains(r#"codex = "~/.codex""#));

    let loaded = ProjectConfig::load_with_paths(None, &project_path)
        .unwrap()
        .expect("config should load");
    assert_eq!(
        loaded.tool_state_dirs.get("codex"),
        Some(&PathBuf::from("~/.codex"))
    );
    assert_eq!(
        loaded.tool_state_dirs.get("claude"),
        Some(&PathBuf::from("~/.claude"))
    );
}

#[test]
fn test_is_tool_enabled_configured_enabled() {
    let mut tools = HashMap::new();
    tools.insert("codex".to_string(), ToolConfig::default());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    assert!(config.is_tool_enabled("codex"));
}

#[test]
fn test_is_tool_enabled_configured_disabled() {
    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: false,
            restrictions: None,
            suppress_notify: true,
            ..Default::default()
        },
    );

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    assert!(!config.is_tool_enabled("codex"));
}

#[test]
fn test_is_tool_enabled_unconfigured_defaults_to_true() {
    let config = ProjectConfig {
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
        preferences: None,
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    assert!(config.is_tool_enabled("codex"));
}

#[test]
fn test_is_tool_configured_in_tiers_detects_presence() {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier3".to_string(),
        TierConfig {
            description: "test".to_string(),
            models: vec![
                "codex/provider/model/medium".to_string(),
                "claude-code/provider/model/high".to_string(),
            ],
            strategy: TierStrategy::default(),

            token_budget: None,
            max_turns: None,
        },
    );

    let config = ProjectConfig {
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
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    assert!(config.is_tool_configured_in_tiers("codex"));
    assert!(config.is_tool_configured_in_tiers("claude-code"));
    assert!(!config.is_tool_configured_in_tiers("gemini-cli"));
}

#[test]
fn test_is_tool_auto_selectable_requires_enabled_and_tier_membership() {
    let mut tools = HashMap::new();
    tools.insert("codex".to_string(), ToolConfig::default());
    tools.insert(
        "claude-code".to_string(),
        ToolConfig {
            enabled: false,
            restrictions: None,
            suppress_notify: true,
            ..Default::default()
        },
    );

    let mut tiers = HashMap::new();
    tiers.insert(
        "tier3".to_string(),
        TierConfig {
            description: "test".to_string(),
            models: vec![
                "codex/provider/model/medium".to_string(),
                "claude-code/provider/model/high".to_string(),
            ],
            strategy: TierStrategy::default(),

            token_budget: None,
            max_turns: None,
        },
    );

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    assert!(config.is_tool_auto_selectable("codex"));
    assert!(!config.is_tool_auto_selectable("claude-code")); // disabled
    assert!(!config.is_tool_auto_selectable("gemini-cli")); // not in tiers
}

#[test]
fn test_can_tool_edit_existing_with_restrictions_false() {
    let mut tools = HashMap::new();
    tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            enabled: true,
            restrictions: Some(ToolRestrictions {
                allow_edit_existing_files: false,
                allow_write_new_files: true,
            }),
            suppress_notify: true,
            ..Default::default()
        },
    );

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    assert!(!config.can_tool_edit_existing("gemini-cli"));
}

#[test]
fn test_can_tool_edit_existing_without_restrictions() {
    let mut tools = HashMap::new();
    tools.insert("codex".to_string(), ToolConfig::default());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    assert!(config.can_tool_edit_existing("codex"));
}

include!("config_tests_split.rs");
