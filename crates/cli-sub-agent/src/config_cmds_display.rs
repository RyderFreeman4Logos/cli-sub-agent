use csa_config::ProjectConfig;

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
