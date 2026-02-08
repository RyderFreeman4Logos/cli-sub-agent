use super::*;
use tempfile::tempdir;

#[test]
fn test_load_nonexistent_returns_none() {
    let dir = tempdir().unwrap();
    // Use load_with_paths to isolate from real ~/.config/csa/config.toml on host.
    let project_path = dir.path().join(".csa").join("config.toml");
    let result = ProjectConfig::load_with_paths(None, &project_path).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_save_and_load_roundtrip() {
    let dir = tempdir().unwrap();

    let mut tools = HashMap::new();
    tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            enabled: true,
            restrictions: Some(ToolRestrictions {
                allow_edit_existing_files: false,
            }),
            suppress_notify: false,
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
        tools,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();

    let loaded = ProjectConfig::load(dir.path()).unwrap();
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();

    assert_eq!(loaded.project.name, "test-project");
    assert_eq!(loaded.project.max_recursion_depth, 5);
    assert!(loaded.tools.contains_key("gemini-cli"));
    assert!(loaded.tools.get("gemini-cli").unwrap().enabled);
}

#[test]
fn test_is_tool_enabled_configured_enabled() {
    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: true,
            restrictions: None,
            suppress_notify: false,
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
        tools,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
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
            suppress_notify: false,
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
        tools,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
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
        tools: HashMap::new(),
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    assert!(config.is_tool_enabled("codex"));
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
            }),
            suppress_notify: false,
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
        tools,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    assert!(!config.can_tool_edit_existing("gemini-cli"));
}

#[test]
fn test_can_tool_edit_existing_without_restrictions() {
    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: true,
            restrictions: None,
            suppress_notify: false,
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
        tools,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    assert!(config.can_tool_edit_existing("codex"));
}

#[test]
fn test_can_tool_edit_existing_unconfigured_defaults_to_true() {
    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    assert!(config.can_tool_edit_existing("codex"));
}

#[test]
fn test_resolve_tier_default_selection() {
    let mut tools = HashMap::new();
    tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            enabled: true,
            restrictions: None,
            suppress_notify: false,
        },
    );
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: true,
            restrictions: None,
            suppress_notify: false,
        },
    );

    let mut tiers = HashMap::new();
    tiers.insert(
        "tier1".to_string(),
        TierConfig {
            description: "Quick tier".to_string(),
            models: vec![
                "gemini-cli/google/gemini-3-flash-preview/xhigh".to_string(),
                "codex/anthropic/claude-opus/high".to_string(),
            ],
        },
    );

    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("default".to_string(), "tier1".to_string());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools,
        tiers,
        tier_mapping,
        aliases: HashMap::new(),
    };

    let result = config.resolve_tier_tool("default");
    assert!(result.is_some());
    let (tool_name, model_spec) = result.unwrap();
    assert_eq!(tool_name, "gemini-cli");
    assert_eq!(model_spec, "gemini-cli/google/gemini-3-flash-preview/xhigh");
}

#[test]
fn test_resolve_tier_fallback_to_tier3() {
    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: true,
            restrictions: None,
            suppress_notify: false,
        },
    );

    let mut tiers = HashMap::new();
    tiers.insert(
        "tier3".to_string(),
        TierConfig {
            description: "Fallback tier".to_string(),
            models: vec!["codex/anthropic/claude-opus/medium".to_string()],
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
        tools,
        tiers,
        tier_mapping: HashMap::new(), // No mapping for "unknown_task"
        aliases: HashMap::new(),
    };

    // Should fallback to tier3
    let result = config.resolve_tier_tool("unknown_task");
    assert!(result.is_some());
    let (tool_name, model_spec) = result.unwrap();
    assert_eq!(tool_name, "codex");
    assert_eq!(model_spec, "codex/anthropic/claude-opus/medium");
}

#[test]
fn test_resolve_tier_skips_disabled_tools() {
    let mut tools = HashMap::new();
    tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            enabled: false, // Disabled
            restrictions: None,
            suppress_notify: false,
        },
    );
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: true,
            restrictions: None,
            suppress_notify: false,
        },
    );

    let mut tiers = HashMap::new();
    tiers.insert(
        "tier1".to_string(),
        TierConfig {
            description: "Test tier".to_string(),
            models: vec![
                "gemini-cli/google/gemini-3-flash-preview/xhigh".to_string(),
                "codex/anthropic/claude-opus/high".to_string(),
            ],
        },
    );

    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("default".to_string(), "tier1".to_string());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools,
        tiers,
        tier_mapping,
        aliases: HashMap::new(),
    };

    // Should skip disabled gemini-cli and select codex
    let result = config.resolve_tier_tool("default");
    assert!(result.is_some());
    let (tool_name, _) = result.unwrap();
    assert_eq!(tool_name, "codex");
}

#[test]
fn test_resolve_alias() {
    let mut aliases = HashMap::new();
    aliases.insert(
        "fast".to_string(),
        "gemini-cli/google/gemini-3-flash-preview/low".to_string(),
    );
    aliases.insert(
        "smart".to_string(),
        "codex/anthropic/claude-opus/xhigh".to_string(),
    );

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases,
    };

    // Resolve alias
    assert_eq!(
        config.resolve_alias("fast"),
        "gemini-cli/google/gemini-3-flash-preview/low"
    );
    assert_eq!(
        config.resolve_alias("smart"),
        "codex/anthropic/claude-opus/xhigh"
    );

    // Non-alias should be returned unchanged
    assert_eq!(
        config.resolve_alias("codex/anthropic/claude-opus/high"),
        "codex/anthropic/claude-opus/high"
    );
}

#[test]
fn test_max_recursion_depth_override() {
    let dir = tempdir().unwrap();

    // Config with custom max_recursion_depth
    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test-project".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 10,
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();

    let loaded = ProjectConfig::load(dir.path()).unwrap();
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();

    assert_eq!(loaded.project.max_recursion_depth, 10);
}

#[test]
fn test_max_recursion_depth_default() {
    let dir = tempdir().unwrap();

    // Config without explicitly setting max_recursion_depth (should use default)
    let config_toml = r#"
[project]
name = "test-project"
created_at = "2024-01-01T00:00:00Z"

[resources]
"#;

    let config_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), config_toml).unwrap();

    let loaded = ProjectConfig::load(dir.path()).unwrap();
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();

    assert_eq!(loaded.project.max_recursion_depth, 5);
}

#[test]
#[ignore] // Only run manually to test actual project config
fn test_load_actual_project_config() {
    // Try to find project root
    let current_dir = std::env::current_dir().unwrap();
    let mut project_root = current_dir.as_path();

    // Walk up until we find .csa/config.toml
    loop {
        let config_path = project_root.join(".csa/config.toml");
        if config_path.exists() {
            println!("Found config at: {}", config_path.display());
            break;
        }
        project_root = match project_root.parent() {
            Some(p) => p,
            None => {
                println!("Could not find .csa/config.toml in parent directories");
                return;
            }
        };
    }

    let result = ProjectConfig::load(project_root);
    assert!(result.is_ok(), "Failed to load config: {:?}", result.err());

    let config = result.unwrap();
    assert!(config.is_some(), "Config should exist");

    let config = config.unwrap();
    println!(
        "✓ Successfully loaded project config: {}",
        config.project.name
    );
    println!("✓ Tiers defined: {}", config.tiers.len());

    for (name, tier_config) in &config.tiers {
        println!(
            "  - {}: {} (models: {})",
            name,
            tier_config.description,
            tier_config.models.len()
        );
        assert!(
            !tier_config.models.is_empty(),
            "Tier {} should have models",
            name
        );
        for model in &tier_config.models {
            let parts: Vec<&str> = model.split('/').collect();
            assert_eq!(
                parts.len(),
                4,
                "Model spec '{}' should have format 'tool/provider/model/budget'",
                model
            );
        }
    }

    println!("✓ Tier mappings defined: {}", config.tier_mapping.len());
    for (task, tier) in &config.tier_mapping {
        println!("  - {} -> {}", task, tier);
        assert!(
            config.tiers.contains_key(tier),
            "Tier mapping {} references undefined tier {}",
            task,
            tier
        );
    }

    println!("✓ All validation checks passed!");
}

#[test]
fn test_schema_version_current_is_ok() {
    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    assert!(config.check_schema_version().is_ok());
}

#[test]
fn test_schema_version_older_is_ok() {
    // Older schemas are backward compatible
    let config = ProjectConfig {
        schema_version: 0,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    assert!(config.check_schema_version().is_ok());
}

#[test]
fn test_schema_version_newer_fails() {
    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION + 1,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    let result = config.check_schema_version();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("newer"));
}

#[test]
fn test_schema_version_default_when_missing() {
    let dir = tempdir().unwrap();

    // Config without schema_version field (should default to CURRENT_SCHEMA_VERSION)
    let config_toml = r#"
[project]
name = "test-project"
created_at = "2024-01-01T00:00:00Z"
"#;

    let config_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), config_toml).unwrap();

    let loaded = ProjectConfig::load(dir.path()).unwrap().unwrap();
    assert_eq!(loaded.schema_version, CURRENT_SCHEMA_VERSION);
    assert!(loaded.check_schema_version().is_ok());
}

#[test]
fn test_merge_tiers_deep_merge() {
    let user_toml: toml::Value = toml::from_str(
        r#"
        schema_version = 1
        [tiers.tier1]
        description = "User tier 1"
        models = ["gemini-cli/google/flash/low"]
        [tiers.tier2]
        description = "User tier 2"
        models = ["codex/openai/gpt/medium"]
    "#,
    )
    .unwrap();

    let project_toml: toml::Value = toml::from_str(
        r#"
        schema_version = 1
        [tiers.tier2]
        description = "Project tier 2 override"
        models = ["claude-code/anthropic/opus/high"]
        [tiers.tier3]
        description = "Project tier 3"
        models = ["codex/openai/o3/xhigh"]
    "#,
    )
    .unwrap();

    let merged = merge_toml_values(user_toml, project_toml);
    let config: ProjectConfig = toml::from_str(&toml::to_string(&merged).unwrap()).unwrap();

    // tier1 from user (untouched)
    assert!(config.tiers.contains_key("tier1"));
    assert_eq!(config.tiers["tier1"].description, "User tier 1");

    // tier2 from project (overridden)
    assert!(config.tiers.contains_key("tier2"));
    assert_eq!(config.tiers["tier2"].description, "Project tier 2 override");
    assert_eq!(config.tiers["tier2"].models.len(), 1);
    assert!(config.tiers["tier2"].models[0].contains("claude-code"));

    // tier3 from project (new)
    assert!(config.tiers.contains_key("tier3"));
}

#[test]
fn test_merge_scalar_overlay_wins() {
    let base: toml::Value = toml::from_str(
        r#"
        schema_version = 1
        [project]
        name = "user-default"
        max_recursion_depth = 3
    "#,
    )
    .unwrap();

    let overlay: toml::Value = toml::from_str(
        r#"
        [project]
        name = "my-project"
    "#,
    )
    .unwrap();

    let merged = merge_toml_values(base, overlay);
    let config: ProjectConfig = toml::from_str(&toml::to_string(&merged).unwrap()).unwrap();

    assert_eq!(config.project.name, "my-project");
    // max_recursion_depth should come from user (base) since project didn't set it
    // After merge, the [project] table merges recursively:
    // base has name + max_recursion_depth, overlay has name only
    // So merged [project] has name from overlay + max_recursion_depth from base
    assert_eq!(config.project.max_recursion_depth, 3);
}

#[test]
fn test_suppress_notify_in_tool_config() {
    let toml_str = r#"
        schema_version = 1
        [tools.codex]
        enabled = true
        suppress_notify = true
        [tools.gemini-cli]
        enabled = true
    "#;
    let config: ProjectConfig = toml::from_str(toml_str).unwrap();

    assert!(config.should_suppress_codex_notify());
    // gemini-cli doesn't have suppress_notify set, should default to false
    assert!(!config.tools["gemini-cli"].suppress_notify);
}

#[test]
fn test_suppress_notify_default_false() {
    let toml_str = r#"
        schema_version = 1
        [tools.codex]
        enabled = true
    "#;
    let config: ProjectConfig = toml::from_str(toml_str).unwrap();
    assert!(!config.should_suppress_codex_notify());
}

#[test]
fn test_user_config_path_returns_some() {
    // On a normal system with HOME set, this should return Some
    let path = ProjectConfig::user_config_path();
    if std::env::var("HOME").is_ok() {
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(p.to_string_lossy().contains("csa"));
        assert!(p.to_string_lossy().contains("config.toml"));
    }
    // In containers without HOME, it's OK to return None
}

#[test]
fn test_user_config_template_is_valid() {
    let template = ProjectConfig::user_config_template();
    // Template should contain key sections
    assert!(template.contains("schema_version"));
    assert!(template.contains("[resources]"));
    assert!(template.contains("suppress_notify"));
    assert!(template.contains("# [tiers."));
}
