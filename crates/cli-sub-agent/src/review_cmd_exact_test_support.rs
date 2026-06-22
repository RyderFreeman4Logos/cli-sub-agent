fn exact_test_configure_codex_cli_review_test_tool(config: &mut csa_config::ProjectConfig) {
    let codex = config.tools.get_mut("codex").expect("codex tool config");
    codex.transport = Some(csa_config::TransportKind::Cli);
    codex.enforcement_mode = Some(csa_config::EnforcementMode::Off);
}

#[test]
fn exact_codex_cli_review_test_tool_preserves_low_memory_projection() {
    let mut config = exact_test_project_config_with_enabled_tools(&["codex"]);
    exact_test_configure_codex_cli_review_test_tool(&mut config);
    assert_eq!(config.sandbox_memory_max_mb("codex"), Some(1024));
    assert_eq!(
        config.tool_enforcement_mode("codex"),
        csa_config::EnforcementMode::Off
    );
}
