use super::*;

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
        session_wait: None,
        preflight: Default::default(),
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
        session_wait: None,
        preflight: Default::default(),
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
        session_wait: None,
        preflight: Default::default(),
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
        session_wait: None,
        preflight: Default::default(),
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
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    assert_eq!(config.resolve_tier_selector("broken"), None);
}

// ---------------------------------------------------------------------------
// Prefix matching tests
// ---------------------------------------------------------------------------

#[test]
fn test_resolve_tier_selector_prefix_unique() {
    let mut tiers = HashMap::new();
    for name in [
        "tier-1-quick",
        "tier-2-standard",
        "tier-3-complex",
        "tier-4-critical",
    ] {
        tiers.insert(
            name.to_string(),
            TierConfig {
                description: "test".to_string(),
                models: vec!["gemini-cli/google/default/xhigh".to_string()],
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        );
    }

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
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    // Unique prefix → resolves
    assert_eq!(
        config.resolve_tier_selector("tier-1"),
        Some("tier-1-quick".to_string()),
    );
    assert_eq!(
        config.resolve_tier_selector("tier-4"),
        Some("tier-4-critical".to_string()),
    );
    // Exact match still works
    assert_eq!(
        config.resolve_tier_selector("tier-2-standard"),
        Some("tier-2-standard".to_string()),
    );
}

#[test]
fn test_resolve_tier_selector_prefix_ambiguous() {
    let mut tiers = HashMap::new();
    // Two tiers share "tier-1" prefix
    for name in ["tier-1-quick", "tier-1-extended"] {
        tiers.insert(
            name.to_string(),
            TierConfig {
                description: "test".to_string(),
                models: vec!["gemini-cli/google/default/xhigh".to_string()],
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        );
    }

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
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    // Ambiguous prefix → None
    assert_eq!(config.resolve_tier_selector("tier-1"), None);
}

#[test]
fn test_resolve_tier_selector_exact_wins_over_prefix() {
    let mut tiers = HashMap::new();
    // "tier-1" is both an exact name AND a prefix of "tier-1-quick"
    for name in ["tier-1", "tier-1-quick"] {
        tiers.insert(
            name.to_string(),
            TierConfig {
                description: "test".to_string(),
                models: vec!["gemini-cli/google/default/xhigh".to_string()],
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        );
    }

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
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    // Exact match takes priority
    assert_eq!(
        config.resolve_tier_selector("tier-1"),
        Some("tier-1".to_string()),
    );
}

// ---------------------------------------------------------------------------
// suggest_tier() tests
// ---------------------------------------------------------------------------

#[test]
fn test_suggest_tier_prefix() {
    let mut tiers = HashMap::new();
    for name in ["tier-1-quick", "tier-2-standard"] {
        tiers.insert(
            name.to_string(),
            TierConfig {
                description: "test".to_string(),
                models: vec!["gemini-cli/google/default/xhigh".to_string()],
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        );
    }

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
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    assert_eq!(
        config.suggest_tier("tier-1"),
        Some("tier-1-quick".to_string()),
    );
    assert_eq!(
        config.suggest_tier("tier-2"),
        Some("tier-2-standard".to_string()),
    );
}

#[test]
fn test_suggest_tier_substring() {
    let mut tiers = HashMap::new();
    for name in ["tier-1-quick", "tier-2-standard"] {
        tiers.insert(
            name.to_string(),
            TierConfig {
                description: "test".to_string(),
                models: vec!["gemini-cli/google/default/xhigh".to_string()],
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        );
    }

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
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    // "quick" is a substring of only "tier-1-quick"
    assert_eq!(
        config.suggest_tier("quick"),
        Some("tier-1-quick".to_string()),
    );
    // "standard" is a substring of only "tier-2-standard"
    assert_eq!(
        config.suggest_tier("standard"),
        Some("tier-2-standard".to_string()),
    );
    // "tier" matches both — no suggestion
    assert_eq!(config.suggest_tier("tier"), None);
}

#[test]
fn test_suggest_tier_no_match() {
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
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    assert_eq!(config.suggest_tier("anything"), None);
}

// ---------------------------------------------------------------------------
// Empty selector regression tests (PR #460 review finding)
// ---------------------------------------------------------------------------

#[test]
fn test_resolve_tier_selector_empty_string_rejected() {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-1-quick".to_string(),
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
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    // Empty string must NOT resolve via prefix matching (regression: PR #460)
    assert_eq!(config.resolve_tier_selector(""), None);
    assert_eq!(config.resolve_tier_selector("  "), None);
    assert_eq!(config.suggest_tier(""), None);
    assert_eq!(config.suggest_tier("  "), None);
}
