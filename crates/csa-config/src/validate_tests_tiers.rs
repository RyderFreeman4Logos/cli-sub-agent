#[test]
fn test_validate_multiple_tiers_all_valid() {
    let dir = tempdir().unwrap();

    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-1-quick".to_string(),
        TierConfig {
            description: "Quick tasks".to_string(),
            models: vec!["gemini-cli/google/gemini-3-flash-preview/xhigh".to_string()],
        },
    );
    tiers.insert(
        "tier-2-standard".to_string(),
        TierConfig {
            description: "Standard tasks".to_string(),
            models: vec!["codex/anthropic/claude-sonnet-4-5/default".to_string()],
        },
    );
    tiers.insert(
        "tier-3-complex".to_string(),
        TierConfig {
            description: "Complex tasks".to_string(),
            models: vec!["claude-code/anthropic/claude-opus-4-6/default".to_string()],
        },
    );

    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("default".to_string(), "tier-2-standard".to_string());
    tier_mapping.insert("quick_question".to_string(), "tier-1-quick".to_string());
    tier_mapping.insert("security_audit".to_string(), "tier-3-complex".to_string());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping,
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
    assert!(result.is_ok());
}

#[test]
fn test_validate_tier_with_multiple_models_all_valid() {
    let dir = tempdir().unwrap();

    let mut tiers = HashMap::new();
    tiers.insert(
        "multi-model-tier".to_string(),
        TierConfig {
            description: "Has multiple models".to_string(),
            models: vec![
                "gemini-cli/google/gemini-3-flash-preview/xhigh".to_string(),
                "codex/anthropic/claude-sonnet-4-5/default".to_string(),
            ],
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
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
    assert!(result.is_ok());
}

#[test]
fn test_validate_tier_with_one_bad_model_in_list() {
    let dir = tempdir().unwrap();

    let mut tiers = HashMap::new();
    tiers.insert(
        "mixed-tier".to_string(),
        TierConfig {
            description: "One good, one bad".to_string(),
            models: vec![
                "gemini-cli/google/gemini-3-flash-preview/xhigh".to_string(),
                "bad-spec".to_string(), // invalid
            ],
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
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("invalid model spec")
    );
}
