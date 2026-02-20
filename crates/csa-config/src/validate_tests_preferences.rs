#[test]
fn test_validate_config_warns_but_passes_on_unknown_tool_priority() {
    let dir = tempdir().unwrap();

    let mut tools = HashMap::new();
    tools.insert("codex".to_string(), ToolConfig::default());

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
        preferences: Some(crate::global::PreferencesConfig {
            tool_priority: vec!["codexx".into(), "codex".into()],
        }),
        session: Default::default(),
    };

    config.save(dir.path()).unwrap();
    // Should pass validation (warn is non-fatal)
    let result = validate_config(dir.path());
    assert!(
        result.is_ok(),
        "unknown tool_priority entry should warn, not fail: {:?}",
        result
    );
}
