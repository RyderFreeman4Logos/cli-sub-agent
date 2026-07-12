//! Project-config compatibility pruning for removed tool references.
//!
//! User/global config stays fail-closed. Project configs are committed policy
//! files, so a stale removed-tool entry can otherwise make a clean checkout
//! unable to run the migrator or review gate.

pub(crate) fn prune_removed_project_refs_in_raw_config(
    raw: &mut toml::Value,
    source: &str,
) -> usize {
    prune_value(raw, source, "$")
}

fn prune_value(raw: &mut toml::Value, source: &str, path: &str) -> usize {
    match raw {
        toml::Value::Table(table) => prune_table(table, source, path),
        toml::Value::Array(items) => prune_array(items, source, path),
        _ => 0,
    }
}

fn prune_table(table: &mut toml::map::Map<String, toml::Value>, source: &str, path: &str) -> usize {
    let keys: Vec<String> = table.keys().cloned().collect();
    let mut pruned = 0;
    for key in keys {
        let child_path = format!("{path}.{key}");
        if let Some(removed_value) = table_removed_value(table, path, &child_path, &key) {
            warn_removed(source, &child_path, &removed_value);
            table.remove(&key);
            pruned += 1;
            continue;
        }
        if let Some(value) = table.get_mut(&key) {
            pruned += prune_value(value, source, &child_path);
        }
    }
    pruned
}

fn table_removed_value(
    table: &toml::map::Map<String, toml::Value>,
    parent_path: &str,
    child_path: &str,
    key: &str,
) -> Option<String> {
    if is_semantic_tool_key_table(parent_path) && csa_core::types::is_removed_tool_name(key) {
        return Some(key.to_owned());
    }
    match table.get(key) {
        Some(toml::Value::String(value)) if is_removed_config_value(child_path, value) => {
            Some(value.clone())
        }
        _ => None,
    }
}

fn prune_array(items: &mut Vec<toml::Value>, source: &str, path: &str) -> usize {
    let mut pruned = 0;
    let mut index = 0;
    while index < items.len() {
        let child_path = format!("{path}[{index}]");
        match removable_array_item(&items[index], &child_path) {
            ArrayRemoval::RemovedTool(value) => {
                warn_removed(source, &child_path, &value);
                items.remove(index);
                pruned += 1;
            }
            ArrayRemoval::Keep => {
                pruned += prune_value(&mut items[index], source, &child_path);
                index += 1;
            }
        }
    }
    pruned
}

enum ArrayRemoval {
    RemovedTool(String),
    Keep,
}

fn removable_array_item(item: &toml::Value, path: &str) -> ArrayRemoval {
    let Some(value) = item.as_str() else {
        return ArrayRemoval::Keep;
    };
    if is_removed_config_value(path, value) {
        return ArrayRemoval::RemovedTool(value.to_owned());
    }
    ArrayRemoval::Keep
}

fn is_removed_config_value(path: &str, value: &str) -> bool {
    if is_semantic_tool_value_path(path) {
        return csa_core::types::is_removed_tool_name(value);
    }
    is_semantic_model_spec_value_path(path) && is_removed_model_spec(value)
}

fn is_semantic_tool_key_table(path: &str) -> bool {
    matches!(path, "$.tools" | "$.resources.initial_estimates")
}

fn is_semantic_tool_value_path(path: &str) -> bool {
    matches!(path, "$.defaults.tool" | "$.review.tool" | "$.debate.tool")
        || path.starts_with("$.review.tool[")
        || path.starts_with("$.debate.tool[")
        || path.starts_with("$.tool_aliases.")
        || path.starts_with("$.preferences.tool_priority[")
}

fn is_semantic_model_spec_value_path(path: &str) -> bool {
    path.starts_with("$.aliases.")
        || path == "$.preferences.primary_writer_spec"
        || is_tier_model_spec_value_path(path)
}

fn is_tier_model_spec_value_path(path: &str) -> bool {
    path.starts_with("$.tiers.") && path.contains(".models[")
}

fn is_removed_model_spec(value: &str) -> bool {
    let tool = value.split_once('/').map_or(value, |(tool, _)| tool);
    csa_core::types::is_removed_tool_name(tool)
}

fn warn_removed(source: &str, path: &str, value: &str) {
    eprintln!(
        "warning: project config {source}: ignoring removed tool reference '{value}' at {path}. {} Action: remove this entry or replace it with codex/claude-code.",
        csa_core::types::removed_tool_error("gemini-cli")
    );
}
