use super::*;
use tempfile::tempdir;

#[test]
fn test_load_nonexistent_returns_none() {
    let dir = tempdir().unwrap();
    // Use load_with_paths to isolate from real ~/.config/cli-sub-agent/config.toml on host.
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
            suppress_notify: true,
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
        preferences: None,
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
    assert!(loaded.tools.contains_key("gemini-cli"));
    assert!(loaded.tools.get("gemini-cli").unwrap().enabled);
}

#[test]
fn test_save_and_load_roundtrip_with_review_override() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test-project".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: Some(crate::global::ReviewConfig {
            tool: "codex".to_string(),
        }),
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
    };

    config.save(dir.path()).unwrap();

    // Use load_with_paths to avoid accidental merge with host user config.
    let project_path = dir.path().join(".csa").join("config.toml");
    let loaded = ProjectConfig::load_with_paths(None, &project_path).unwrap();
    let loaded = loaded.unwrap();

    assert_eq!(loaded.review.unwrap().tool, "codex");
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
        preferences: None,
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
        preferences: None,
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
        preferences: None,
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
        preferences: None,
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
        preferences: None,
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
        preferences: None,
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
        preferences: None,
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
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
    };

    assert!(config.can_tool_edit_existing("codex"));
}

#[test]
fn test_resolve_tier_default_selection() {
    let mut tools = HashMap::new();
    tools.insert("gemini-cli".to_string(), ToolConfig::default());
    tools.insert("codex".to_string(), ToolConfig::default());

    let mut tiers = HashMap::new();
    tiers.insert(
        "tier1".to_string(),
        TierConfig {
            description: "Quick tier".to_string(),
            models: vec![
                "gemini-cli/google/gemini-3-flash-preview/xhigh".to_string(),
                "codex/anthropic/claude-opus/high".to_string(),
            ],
            token_budget: None,
            max_turns: None,
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
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers,
        tier_mapping,
        aliases: HashMap::new(),
        preferences: None,
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
    tools.insert("codex".to_string(), ToolConfig::default());

    let mut tiers = HashMap::new();
    tiers.insert(
        "tier3".to_string(),
        TierConfig {
            description: "Fallback tier".to_string(),
            models: vec!["codex/anthropic/claude-opus/medium".to_string()],
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
        tier_mapping: HashMap::new(), // No mapping for "unknown_task"
        aliases: HashMap::new(),
        preferences: None,
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
            suppress_notify: true,
            ..Default::default()
        },
    );
    tools.insert("codex".to_string(), ToolConfig::default());

    let mut tiers = HashMap::new();
    tiers.insert(
        "tier1".to_string(),
        TierConfig {
            description: "Test tier".to_string(),
            models: vec![
                "gemini-cli/google/gemini-3-flash-preview/xhigh".to_string(),
                "codex/anthropic/claude-opus/high".to_string(),
            ],
            token_budget: None,
            max_turns: None,
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
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers,
        tier_mapping,
        aliases: HashMap::new(),
        preferences: None,
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
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases,
        preferences: None,
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
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
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
    assert_eq!(loaded.resources.idle_timeout_seconds, 120);
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
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
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
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
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
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
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

#[path = "config_tests_tier_whitelist.rs"]
mod tier_whitelist;
