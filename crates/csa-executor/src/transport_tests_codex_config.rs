#[test]
fn test_build_summary_preserves_codex_config_parse_diagnostic() {
    let stderr = "Error loading config.toml:\n/home/obj/.codex/config.toml:21:1: duplicate key\n21 | service_tier = \"default\"\n   | ^^^^^^^^^^^^\n";
    let summary = super::build_summary("21 | service_tier = \"default\"\n", stderr, 1);
    assert!(summary.contains("Error loading config.toml"), "summary: {summary}");
    assert!(summary.contains("duplicate key"), "summary: {summary}");
}
