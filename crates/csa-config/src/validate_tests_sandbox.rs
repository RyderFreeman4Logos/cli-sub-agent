// Sandbox resource validation tests (split from validate_tests.rs).

#[test]
fn test_validate_memory_max_mb_too_low() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            memory_max_mb: Some(100),
            ..Default::default()
        },
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("memory_max_mb must be >= 256")
    );
}

#[test]
fn test_validate_memory_max_mb_at_minimum() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            memory_max_mb: Some(256),
            ..Default::default()
        },
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
    assert!(result.is_ok(), "memory_max_mb 256 should be valid");
}

#[test]
fn test_validate_pids_max_too_low() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            pids_max: Some(5),
            ..Default::default()
        },
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("pids_max must be >= 10")
    );
}

#[test]
fn test_validate_pids_max_at_minimum() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            pids_max: Some(10),
            ..Default::default()
        },
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
    assert!(result.is_ok(), "pids_max 10 should be valid");
}

#[test]
fn test_validate_node_heap_limit_mb_too_low_in_resources() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            node_heap_limit_mb: Some(256),
            ..Default::default()
        },
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("resources.node_heap_limit_mb must be >= 512")
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
        tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("tools.claude-code.node_heap_limit_mb must be >= 512")
    );
}
