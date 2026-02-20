/// Warn about deprecated config keys that serde silently ignores.
pub(crate) fn warn_deprecated_keys(raw: &toml::Value, source: &str) {
    if let Some(resources) = raw.get("resources") {
        if resources.get("min_free_swap_mb").is_some() {
            eprintln!(
                "warning: config '{}': 'resources.min_free_swap_mb' is deprecated and ignored. \
                 Use 'resources.min_free_memory_mb' (combined physical + swap threshold) instead.",
                source
            );
        }
    }
}

/// Deep merge two TOML values. Overlay wins for non-table values.
/// Tables are merged recursively (project-level keys override user-level keys).
pub(crate) fn merge_toml_values(base: toml::Value, overlay: toml::Value) -> toml::Value {
    match (base, overlay) {
        (toml::Value::Table(mut base_map), toml::Value::Table(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let merged_val = match base_map.remove(&key) {
                    Some(base_val) => merge_toml_values(base_val, overlay_val),
                    None => overlay_val,
                };
                base_map.insert(key, merged_val);
            }
            toml::Value::Table(base_map)
        }
        (_, overlay) => overlay,
    }
}

/// Re-apply `tools.*.enabled = false` from the global config into a merged
/// TOML value.  This ensures that global disablement is a hard override:
/// project configs cannot set a globally-disabled tool back to `enabled = true`.
pub(crate) fn enforce_global_tool_disables(global: &toml::Value, merged: &mut toml::Value) {
    let global_tools = match global.get("tools").and_then(|t| t.as_table()) {
        Some(t) => t,
        None => return,
    };
    let merged_tools = match merged.get_mut("tools").and_then(|t| t.as_table_mut()) {
        Some(t) => t,
        None => return,
    };

    for (tool_name, global_tool_val) in global_tools {
        let globally_disabled =
            global_tool_val.get("enabled").and_then(|v| v.as_bool()) == Some(false);
        if !globally_disabled {
            continue;
        }
        // Force `enabled = false` in the merged config for this tool.
        if let Some(merged_tool) = merged_tools.get_mut(tool_name) {
            if let Some(table) = merged_tool.as_table_mut() {
                table.insert("enabled".to_string(), toml::Value::Boolean(false));
            }
        }
    }
}
