#[test]
fn test_validate_per_tool_required_enforcement_with_global_memory_passes() {
    let dir = tempdir().unwrap();

    let mut tools = HashMap::new();
    tools.insert(
        "claude-code".to_string(),
        ToolConfig {
            enforcement_mode: Some(crate::config::EnforcementMode::Required),
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
        resources: ResourcesConfig {
            memory_max_mb: Some(4096),
            ..Default::default()
        },
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
    let project_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &project_path);
    assert!(
        result.is_ok(),
        "Required + global memory_max_mb should pass"
    );
}

#[test]
fn test_validate_node_heap_limit_mb_too_low_in_tool() {
    let dir = tempdir().unwrap();

    let mut tools = HashMap::new();
    tools.insert(
        "claude-code".to_string(),
        ToolConfig {
            node_heap_limit_mb: Some(128),
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

    config.save(dir.path()).unwrap();
    let config_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &config_path);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("tools.claude-code.node_heap_limit_mb must be >= 512")
    );
}

#[test]
fn test_validate_soft_limit_percent_zero_rejected() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            soft_limit_percent: Some(0),
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
    let config_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &config_path);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("soft_limit_percent must be 1-100")
    );
}

#[test]
fn test_validate_soft_limit_percent_over_100_rejected() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            soft_limit_percent: Some(101),
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
    let config_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &config_path);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("soft_limit_percent must be 1-100")
    );
}

#[test]
fn test_validate_soft_limit_percent_valid() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            soft_limit_percent: Some(80),
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
    let config_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &config_path);
    assert!(result.is_ok(), "soft_limit_percent 80 should be valid");
}

#[test]
fn test_validate_memory_monitor_interval_zero_rejected() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            memory_monitor_interval_seconds: Some(0),
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
    let config_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &config_path);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("memory_monitor_interval_seconds must be >= 1")
    );
}

#[test]
fn test_validate_memory_monitor_interval_valid() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            memory_monitor_interval_seconds: Some(5),
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
    let config_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &config_path);
    assert!(result.is_ok(), "memory_monitor_interval_seconds 5 should be valid");
}
