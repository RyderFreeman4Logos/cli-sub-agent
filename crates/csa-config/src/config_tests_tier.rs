use super::*;

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
            strategy: TierStrategy::default(),

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
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
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
        tier_mapping: HashMap::new(), // No mapping for "unknown_task"
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
            strategy: TierStrategy::default(),

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
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
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
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
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

// ── enabled_tier_models filtering ────────────────────────────────────

#[test]
fn enabled_tier_models_returns_all_when_no_tools_disabled() {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-1".to_string(),
        TierConfig {
            description: "test".to_string(),
            models: vec![
                "codex/openai/o3/high".to_string(),
                "claude-code/anthropic/default/xhigh".to_string(),
            ],
            strategy: TierStrategy::default(),

            token_budget: None,
            max_turns: None,
        },
    );

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(), // no explicit tool config → all enabled by default
        review: None,
        debate: None,
        tiers,
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

    let models = config.enabled_tier_models("tier-1");
    assert_eq!(models.len(), 2);
    assert!(models.contains(&"codex/openai/o3/high".to_string()));
    assert!(models.contains(&"claude-code/anthropic/default/xhigh".to_string()));
}

#[test]
fn enabled_tier_models_excludes_disabled_tool() {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-3".to_string(),
        TierConfig {
            description: "complex".to_string(),
            models: vec![
                "codex/openai/gpt-5.3-codex/xhigh".to_string(),
                "claude-code/anthropic/default/xhigh".to_string(),
                "gemini-cli/google/gemini-2.5-pro/high".to_string(),
            ],
            strategy: TierStrategy::default(),

            token_budget: None,
            max_turns: None,
        },
    );

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
        tiers,
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

    let models = config.enabled_tier_models("tier-3");
    assert_eq!(models.len(), 2);
    assert!(!models.iter().any(|m| m.starts_with("codex/")));
    assert!(models.contains(&"claude-code/anthropic/default/xhigh".to_string()));
    assert!(models.contains(&"gemini-cli/google/gemini-2.5-pro/high".to_string()));
}

#[test]
fn enabled_tier_models_returns_empty_for_unknown_tier() {
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

    assert!(config.enabled_tier_models("nonexistent").is_empty());
}

#[test]
fn enabled_tier_models_returns_empty_when_all_tools_disabled() {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-1".to_string(),
        TierConfig {
            description: "test".to_string(),
            models: vec![
                "codex/openai/o3/high".to_string(),
                "claude-code/anthropic/default/xhigh".to_string(),
            ],
            strategy: TierStrategy::default(),

            token_budget: None,
            max_turns: None,
        },
    );

    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: false,
            ..Default::default()
        },
    );
    tools.insert(
        "claude-code".to_string(),
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
        tiers,
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

    assert!(config.enabled_tier_models("tier-1").is_empty());
}

// ── resolve_tier_tool_filtered tests ───────────────────────────────

#[test]
fn filtered_skips_restricted_tool_when_needs_edit() {
    let mut tools = HashMap::new();
    tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            restrictions: Some(ToolRestrictions {
                allow_edit_existing_files: false,
                allow_write_new_files: true,
            }),
            ..Default::default()
        },
    );
    tools.insert("codex".to_string(), ToolConfig::default());

    let mut tiers = HashMap::new();
    tiers.insert(
        "tier3".to_string(),
        TierConfig {
            description: "test".to_string(),
            models: vec![
                "gemini-cli/google/gemini-2.5-pro/xhigh".to_string(),
                "codex/openai/o4-mini/0".to_string(),
            ],
            strategy: TierStrategy::default(),

            token_budget: None,
            max_turns: None,
        },
    );

    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("default".to_string(), "tier3".to_string());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers,
        tier_mapping,
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

    // needs_edit=true → should skip gemini-cli, select codex
    let result = config.resolve_tier_tool_filtered("default", true);
    assert!(result.is_some());
    let (tool, _) = result.unwrap();
    assert_eq!(tool, "codex");

    // needs_edit=false → should select gemini-cli (first enabled)
    let result = config.resolve_tier_tool_filtered("default", false);
    assert!(result.is_some());
    let (tool, _) = result.unwrap();
    assert_eq!(tool, "gemini-cli");
}

#[test]
fn filtered_returns_none_when_all_restricted_and_needs_edit() {
    let mut tools = HashMap::new();
    tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            restrictions: Some(ToolRestrictions {
                allow_edit_existing_files: false,
                allow_write_new_files: true,
            }),
            ..Default::default()
        },
    );

    let mut tiers = HashMap::new();
    tiers.insert(
        "tier3".to_string(),
        TierConfig {
            description: "test".to_string(),
            models: vec!["gemini-cli/google/gemini-2.5-pro/xhigh".to_string()],
            strategy: TierStrategy::default(),

            token_budget: None,
            max_turns: None,
        },
    );

    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("default".to_string(), "tier3".to_string());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers,
        tier_mapping,
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

    let result = config.resolve_tier_tool_filtered("default", true);
    assert!(result.is_none());
}

// --- resolve_tier_selector tests ---

#[test]
fn test_resolve_tier_selector_direct_tier() {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier1".to_string(),
        TierConfig {
            description: "test".to_string(),
            models: vec!["gemini-cli/google/default/xhigh".to_string()],
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
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
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    assert_eq!(
        config.resolve_tier_selector("tier1"),
        Some("tier1".to_string())
    );
}

#[test]
fn test_resolve_tier_selector_alias() {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-2-standard".to_string(),
        TierConfig {
            description: "test".to_string(),
            models: vec!["gemini-cli/google/default/xhigh".to_string()],
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("default".to_string(), "tier-2-standard".to_string());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping,
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

    assert_eq!(
        config.resolve_tier_selector("default"),
        Some("tier-2-standard".to_string())
    );
}

#[test]
fn test_resolve_tier_selector_direct_wins_on_collision() {
    let mut tiers = HashMap::new();
    // A tier named "default" exists directly
    tiers.insert(
        "default".to_string(),
        TierConfig {
            description: "direct tier".to_string(),
            models: vec!["gemini-cli/google/default/xhigh".to_string()],
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    tiers.insert(
        "tier-2-standard".to_string(),
        TierConfig {
            description: "standard".to_string(),
            models: vec!["codex/openai/gpt-5/xhigh".to_string()],
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    let mut tier_mapping = HashMap::new();
    // tier_mapping also maps "default" to a different tier
    tier_mapping.insert("default".to_string(), "tier-2-standard".to_string());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping,
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

    // Direct tier name wins over mapping alias
    assert_eq!(
        config.resolve_tier_selector("default"),
        Some("default".to_string())
    );
}

#[test]
fn test_resolve_tier_selector_nonexistent() {
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

    assert_eq!(config.resolve_tier_selector("unknown"), None);
}

#[test]
fn test_resolve_tier_selector_alias_to_nonexistent_tier() {
    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("broken".to_string(), "nonexistent-tier".to_string());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping,
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

    assert_eq!(config.resolve_tier_selector("broken"), None);
}
