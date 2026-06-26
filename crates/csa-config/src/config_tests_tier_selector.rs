use super::*;

// --- resolve_tier_selector tests ---

#[test]
fn test_resolve_tier_selector_direct_tier() {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier1".to_string(),
        TierConfig {
            description: "test".to_string(),
            models: vec!["codex/openai/gpt-5.5/xhigh".to_string()],
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
            models: vec!["codex/openai/gpt-5.5/xhigh".to_string()],
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
            models: vec!["codex/openai/gpt-5.5/xhigh".to_string()],
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
                models: vec!["codex/openai/gpt-5.5/xhigh".to_string()],
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

    // Unique prefix → resolves
    assert_eq!(
        config.resolve_tier_selector("tier1"),
        Some("tier-1-quick".to_string()),
    );
    assert_eq!(
        config.resolve_tier_selector("tier-1"),
        Some("tier-1-quick".to_string()),
    );
    assert_eq!(
        config.resolve_tier_selector("tier2"),
        Some("tier-2-standard".to_string()),
    );
    assert_eq!(
        config.resolve_tier_selector("tier3"),
        Some("tier-3-complex".to_string()),
    );
    assert_eq!(
        config.resolve_tier_selector("tier4"),
        Some("tier-4-critical".to_string()),
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
                models: vec!["codex/openai/gpt-5.5/xhigh".to_string()],
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

    // Numeric shorthand picks the first deterministic prefix match.
    assert_eq!(
        config.resolve_tier_selector("tier-1"),
        Some("tier-1-extended".to_string()),
    );
    // Non-shorthand ambiguous prefix → None
    assert_eq!(config.resolve_tier_selector("tier-1-"), None);
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
                models: vec!["codex/openai/gpt-5.5/xhigh".to_string()],
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
                models: vec!["codex/openai/gpt-5.5/xhigh".to_string()],
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
                models: vec!["codex/openai/gpt-5.5/xhigh".to_string()],
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
            models: vec!["codex/openai/gpt-5.5/xhigh".to_string()],
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

    // Empty string must NOT resolve via prefix matching (regression: PR #460)
    assert_eq!(config.resolve_tier_selector(""), None);
    assert_eq!(config.resolve_tier_selector("  "), None);
    assert_eq!(config.suggest_tier(""), None);
    assert_eq!(config.suggest_tier("  "), None);
}

// ---------------------------------------------------------------------------
// Compound tier-tool selector tests (#1441)
// ---------------------------------------------------------------------------

fn compound_tier_fixture(tool_aliases: HashMap<String, String>) -> ProjectConfig {
    let mut tiers = HashMap::new();
    for name in ["tier-3-complex", "tier-4-critical"] {
        tiers.insert(
            name.to_string(),
            TierConfig {
                description: "test".to_string(),
                models: vec!["codex/openai/gpt-5.5/xhigh".to_string()],
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        );
    }
    ProjectConfig {
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
        tool_aliases,
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
    }
}

#[test]
fn test_try_parse_compound_canonical_tool_suffix() {
    let config = compound_tier_fixture(HashMap::new());
    assert_eq!(
        config.try_parse_compound_tier_tool("tier-4-critical-codex"),
        Some((
            "tier-4-critical".to_string(),
            csa_core::types::ToolName::Codex
        )),
    );
}

#[test]
fn test_try_parse_compound_multi_hyphen_tool_suffix() {
    let config = compound_tier_fixture(HashMap::new());
    // `code` alone is not a tool; `claude-code` matches on the second iteration.
    assert_eq!(
        config.try_parse_compound_tier_tool("tier-4-critical-claude-code"),
        Some((
            "tier-4-critical".to_string(),
            csa_core::types::ToolName::ClaudeCode,
        )),
    );
}

#[test]
fn test_try_parse_compound_builtin_alias_suffix() {
    let config = compound_tier_fixture(HashMap::new());
    // `claude` → ClaudeCode (built-in alias from ToolArg::from_str).
    assert_eq!(
        config.try_parse_compound_tier_tool("tier-4-critical-claude"),
        Some((
            "tier-4-critical".to_string(),
            csa_core::types::ToolName::ClaudeCode,
        )),
    );
    // `gemini` was a legacy alias for the removed gemini-cli integration and
    // must not parse as a tier suffix anymore.
    assert_eq!(
        config.try_parse_compound_tier_tool("tier-4-critical-gemini"),
        None,
    );
}

#[test]
fn test_try_parse_compound_user_defined_alias_suffix() {
    let mut tool_aliases = HashMap::new();
    tool_aliases.insert("cx".to_string(), "codex".to_string());
    let config = compound_tier_fixture(tool_aliases);
    assert_eq!(
        config.try_parse_compound_tier_tool("tier-4-critical-cx"),
        Some((
            "tier-4-critical".to_string(),
            csa_core::types::ToolName::Codex
        )),
    );
}

#[test]
fn test_try_parse_compound_with_numeric_tier_prefix() {
    let config = compound_tier_fixture(HashMap::new());
    // Numeric shorthand: "tier-4" resolves to "tier-4-critical" via prefix matching.
    assert_eq!(
        config.try_parse_compound_tier_tool("tier-4-codex"),
        Some((
            "tier-4-critical".to_string(),
            csa_core::types::ToolName::Codex
        )),
    );
}

#[test]
fn test_try_parse_compound_no_compound_for_direct_tier() {
    // Callers should check resolve_tier_selector first; this method does NOT
    // re-resolve direct tier names (which would shadow tier names that happen
    // to contain a tool suffix). It only parses compound forms.
    let config = compound_tier_fixture(HashMap::new());
    // "tier-4-critical" itself does NOT contain a recognizable tool suffix,
    // so compound parsing returns None and the caller's direct lookup wins.
    assert_eq!(config.try_parse_compound_tier_tool("tier-4-critical"), None,);
}

#[test]
fn test_try_parse_compound_unknown_tier_prefix() {
    let config = compound_tier_fixture(HashMap::new());
    // Tool suffix matches, but no configured tier matches the prefix.
    assert_eq!(
        config.try_parse_compound_tier_tool("nonexistent-codex"),
        None,
    );
}

#[test]
fn test_try_parse_compound_unknown_tool_suffix() {
    let config = compound_tier_fixture(HashMap::new());
    // Prefix is a real tier but suffix is not a tool / not an alias.
    assert_eq!(
        config.try_parse_compound_tier_tool("tier-4-critical-nope"),
        None,
    );
}

#[test]
fn test_try_parse_compound_no_hyphen() {
    let config = compound_tier_fixture(HashMap::new());
    assert_eq!(config.try_parse_compound_tier_tool("codex"), None);
    assert_eq!(config.try_parse_compound_tier_tool(""), None);
}

#[test]
fn test_try_parse_compound_skips_auto_and_any_available() {
    let config = compound_tier_fixture(HashMap::new());
    // `auto` and `any-available` are not specific tools, even though they parse.
    assert_eq!(
        config.try_parse_compound_tier_tool("tier-4-critical-auto"),
        None,
    );
    assert_eq!(
        config.try_parse_compound_tier_tool("tier-4-critical-any-available"),
        None,
    );
}
