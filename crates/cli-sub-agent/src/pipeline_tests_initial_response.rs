use super::*;

// ---------------------------------------------------------------------------
// resolve_initial_response_timeout_seconds
// ---------------------------------------------------------------------------

#[test]
fn test_resolve_initial_response_timeout_cli_override() {
    // CLI override takes precedence over config default.
    assert_eq!(
        resolve_initial_response_timeout_seconds(None, Some(60)),
        Some(60)
    );
}

#[test]
fn test_resolve_initial_response_timeout_cli_override_over_config() {
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig {
            initial_response_timeout_seconds: Some(120),
            ..Default::default()
        },
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
    // CLI=60 overrides config=120.
    assert_eq!(
        resolve_initial_response_timeout_seconds(Some(&cfg), Some(60)),
        Some(60)
    );
}

#[test]
fn test_resolve_initial_response_timeout_zero_disables() {
    // 0 means explicitly disabled → returns None.
    assert_eq!(
        resolve_initial_response_timeout_seconds(None, Some(0)),
        None
    );
}

#[test]
fn test_resolve_initial_response_timeout_config_zero_disables() {
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig {
            initial_response_timeout_seconds: Some(0),
            ..Default::default()
        },
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
    // Config=0 → disabled.
    assert_eq!(
        resolve_initial_response_timeout_seconds(Some(&cfg), None),
        None
    );
}

#[test]
fn test_resolve_initial_response_timeout_no_config_no_cli() {
    // No config, no CLI → None (function returns what config provides).
    assert_eq!(resolve_initial_response_timeout_seconds(None, None), None);
}

#[test]
fn test_resolve_initial_response_timeout_uses_config_value() {
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig {
            initial_response_timeout_seconds: Some(90),
            ..Default::default()
        },
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
    // Config=90, no CLI → Some(90).
    assert_eq!(
        resolve_initial_response_timeout_seconds(Some(&cfg), None),
        Some(90)
    );
}

// ---------------------------------------------------------------------------
// resolve_initial_response_timeout (idle-timeout–aware variant)
// ---------------------------------------------------------------------------

#[test]
fn test_resolve_initial_response_timeout_disabled_when_idle_timeout_explicit() {
    // User set --idle-timeout but NOT --initial-response-timeout → disabled.
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig {
            initial_response_timeout_seconds: Some(120),
            ..Default::default()
        },
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
    // cli_idle_timeout=Some(1200), cli_initial_response_timeout=None → disabled.
    assert_eq!(
        resolve_initial_response_timeout(Some(&cfg), None, Some(1200)),
        None
    );
}

#[test]
fn test_resolve_initial_response_timeout_kept_when_both_explicit() {
    // User set both --idle-timeout AND --initial-response-timeout → both respected.
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig {
            initial_response_timeout_seconds: Some(120),
            ..Default::default()
        },
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
    // Both explicit → initial_response_timeout=60 wins.
    assert_eq!(
        resolve_initial_response_timeout(Some(&cfg), Some(60), Some(1200)),
        Some(60)
    );
}

#[test]
fn test_resolve_initial_response_timeout_falls_through_without_idle_timeout() {
    // No --idle-timeout → falls through to normal resolution.
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig {
            initial_response_timeout_seconds: Some(120),
            ..Default::default()
        },
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
    // No cli_idle_timeout → config default applies.
    assert_eq!(
        resolve_initial_response_timeout(Some(&cfg), None, None),
        Some(120)
    );
}
