use anyhow::Result;
use csa_config::ProjectConfig;

pub(super) fn build_execution_toml(execution: &csa_config::ExecutionConfig) -> toml::Value {
    let mut table = toml::map::Map::new();
    table.insert(
        "min_timeout_seconds".to_string(),
        toml::Value::Integer(execution.min_timeout_seconds as i64),
    );
    table.insert(
        "auto_weave_upgrade".to_string(),
        toml::Value::Boolean(execution.auto_weave_upgrade),
    );
    toml::Value::Table(table)
}

fn snapshot_trigger_name(trigger: csa_config::SnapshotTrigger) -> &'static str {
    match trigger {
        csa_config::SnapshotTrigger::PostRun => "post-run",
        csa_config::SnapshotTrigger::ToolCompleted => "tool-completed",
    }
}

fn build_vcs_toml(vcs: &csa_config::VcsConfig) -> Result<toml::Value> {
    let serialized = toml::Value::try_from(vcs.clone())?;
    let mut table = serialized
        .as_table()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("serialized vcs config was not a TOML table"))?;
    table.insert(
        "auto_snapshot".to_string(),
        toml::Value::Boolean(vcs.auto_snapshot),
    );
    table.insert(
        "snapshot_trigger".to_string(),
        toml::Value::String(snapshot_trigger_name(vcs.snapshot_trigger).to_string()),
    );
    Ok(toml::Value::Table(table))
}

pub(super) fn build_project_display_toml(config: &ProjectConfig) -> Result<toml::Value> {
    let mut root = toml::Value::try_from(config.clone())?;
    let root_table = root
        .as_table_mut()
        .expect("serialized project config should be a TOML table");
    root_table.insert(
        "execution".to_string(),
        build_execution_toml(&config.execution),
    );
    root_table.insert("vcs".to_string(), build_vcs_toml(&config.vcs)?);
    inject_resolved_tool_transports_toml(root_table, config);
    Ok(root)
}

pub(super) fn build_project_display_json(config: &ProjectConfig) -> Result<serde_json::Value> {
    let mut root = serde_json::to_value(config)?;
    let root_object = root
        .as_object_mut()
        .expect("serialized project config should be a JSON object");
    root_object.insert(
        "execution".to_string(),
        serde_json::json!({
            "min_timeout_seconds": config.execution.min_timeout_seconds,
            "auto_weave_upgrade": config.execution.auto_weave_upgrade,
        }),
    );
    root_object.insert(
        "vcs".to_string(),
        serde_json::json!({
            "backend": config.vcs.backend,
            "colocated_default": config.vcs.colocated_default,
            "auto_snapshot": config.vcs.auto_snapshot,
            "snapshot_trigger": snapshot_trigger_name(config.vcs.snapshot_trigger),
        }),
    );
    inject_resolved_tool_transports_json(root_object, config);
    Ok(root)
}

pub(super) fn inject_resolved_tool_transports_toml(
    root: &mut toml::map::Map<String, toml::Value>,
    config: &ProjectConfig,
) {
    let Some(tools) = root.get_mut("tools").and_then(toml::Value::as_table_mut) else {
        return;
    };

    for (tool_name, tool_config) in &config.tools {
        let Some(resolved_transport) = tool_config.resolve_transport(tool_name) else {
            continue;
        };
        let Some(tool_table) = tools.get_mut(tool_name).and_then(toml::Value::as_table_mut) else {
            continue;
        };
        tool_table.insert(
            "transport".to_string(),
            toml::Value::String(
                match resolved_transport {
                    csa_config::TransportKind::Auto => "auto",
                    csa_config::TransportKind::Cli => "cli",
                    csa_config::TransportKind::Acp => "acp",
                }
                .to_string(),
            ),
        );
    }
}

pub(super) fn inject_resolved_tool_transports_json(
    root: &mut serde_json::Map<String, serde_json::Value>,
    config: &ProjectConfig,
) {
    let Some(tools) = root
        .get_mut("tools")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };

    for (tool_name, tool_config) in &config.tools {
        let Some(resolved_transport) = tool_config.resolve_transport(tool_name) else {
            continue;
        };
        let Some(tool_object) = tools
            .get_mut(tool_name)
            .and_then(serde_json::Value::as_object_mut)
        else {
            continue;
        };
        tool_object.insert(
            "transport".to_string(),
            serde_json::Value::String(
                match resolved_transport {
                    csa_config::TransportKind::Auto => "auto",
                    csa_config::TransportKind::Cli => "cli",
                    csa_config::TransportKind::Acp => "acp",
                }
                .to_string(),
            ),
        );
    }
}
