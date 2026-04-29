use super::*;

#[test]
fn build_project_display_toml_keeps_effective_vcs_snapshot_defaults_visible() {
    let config: ProjectConfig = toml::from_str("schema_version = 1\n").unwrap();
    let root = build_project_display_toml(&config).unwrap();

    assert_eq!(
        root.get("vcs")
            .and_then(|value| value.get("auto_snapshot"))
            .and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        root.get("vcs")
            .and_then(|value| value.get("auto_aggregate"))
            .and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        root.get("vcs")
            .and_then(|value| value.get("aggregate_message_template"))
            .and_then(toml::Value::as_str),
        Some("csa: {session_id} ({count} snapshots)")
    );
    assert_eq!(
        root.get("vcs")
            .and_then(|value| value.get("snapshot_trigger"))
            .and_then(toml::Value::as_str),
        Some("post-run")
    );
}

#[test]
fn build_project_display_toml_keeps_explicit_vcs_snapshot_values_visible() {
    let config: ProjectConfig = toml::from_str(
        r#"
schema_version = 1

[vcs]
auto_snapshot = true
auto_aggregate = false
aggregate_message_template = "custom {session_id} {count}"
snapshot_trigger = "tool-completed"
"#,
    )
    .unwrap();
    let root = build_project_display_toml(&config).unwrap();

    assert_eq!(
        root.get("vcs")
            .and_then(|value| value.get("auto_snapshot"))
            .and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        root.get("vcs")
            .and_then(|value| value.get("auto_aggregate"))
            .and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        root.get("vcs")
            .and_then(|value| value.get("aggregate_message_template"))
            .and_then(toml::Value::as_str),
        Some("custom {session_id} {count}")
    );
    assert_eq!(
        root.get("vcs")
            .and_then(|value| value.get("snapshot_trigger"))
            .and_then(toml::Value::as_str),
        Some("tool-completed")
    );
}
