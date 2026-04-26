use super::*;

#[test]
fn build_project_display_toml_resolves_tool_transport_for_display() {
    let config: ProjectConfig = toml::from_str(
        r#"
schema_version = 1

[tools.codex]
transport = "auto"
"#,
    )
    .unwrap();
    let rendered = toml::to_string_pretty(&build_project_display_toml(&config).unwrap()).unwrap();

    assert!(rendered.contains("[tools.codex]"));
    // codex now defaults to CLI transport (#760 / #1128 transport flip).
    assert!(rendered.contains("transport = \"cli\""));
}

#[test]
fn build_project_display_json_resolves_tool_transport_for_display() {
    let config: ProjectConfig = toml::from_str(
        r#"
schema_version = 1

[tools.codex]
transport = "auto"
"#,
    )
    .unwrap();
    let rendered = build_project_display_json(&config).unwrap();

    assert_eq!(
        rendered
            .get("tools")
            .and_then(|value| value.get("codex"))
            .and_then(|value| value.get("transport"))
            .and_then(|value| value.as_str()),
        // codex now defaults to CLI transport (#760 / #1128 transport flip).
        Some("cli")
    );
}
