//! Project-config compatibility pruning for stale tool/model references.
//!
//! User/global config stays fail-closed. Project configs are committed policy
//! files, so a stale removed-tool entry can otherwise make a clean checkout
//! unable to run the migrator or review gate.

pub(crate) fn prune_removed_or_catalog_stale_project_refs_in_raw_config(
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
    let can_prune_stale = is_tier_models_array_path(path) && has_valid_tier_fallback(items);
    let mut index = 0;
    while index < items.len() {
        let child_path = format!("{path}[{index}]");
        match removable_array_item(&items[index], &child_path, can_prune_stale) {
            ArrayRemoval::RemovedTool(value) => {
                warn_removed(source, &child_path, &value);
                items.remove(index);
                pruned += 1;
            }
            ArrayRemoval::StaleModel { value, reason } => {
                warn_stale_model(source, &child_path, &value, &reason);
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
    StaleModel { value: String, reason: String },
    Keep,
}

fn removable_array_item(item: &toml::Value, path: &str, can_prune_stale: bool) -> ArrayRemoval {
    let Some(value) = item.as_str() else {
        return ArrayRemoval::Keep;
    };
    if is_removed_config_value(path, value) {
        return ArrayRemoval::RemovedTool(value.to_owned());
    }
    let stale_reason = can_prune_stale
        .then(|| catalog_stale_tier_model_reason(path, value))
        .flatten();
    if let Some(reason) = stale_reason {
        return ArrayRemoval::StaleModel {
            value: value.to_owned(),
            reason,
        };
    }
    ArrayRemoval::Keep
}

fn has_valid_tier_fallback(items: &[toml::Value]) -> bool {
    items
        .iter()
        .filter_map(toml::Value::as_str)
        .any(is_valid_model_spec)
}

fn is_valid_model_spec(value: &str) -> bool {
    let parts: Vec<&str> = value.split('/').collect();
    parts.len() == 4
        && is_known_active_tool(parts[0])
        && is_known_provider(parts[0], parts[1])
        && is_known_model(parts[0], parts[1], parts[2])
        && csa_core::thinking_budget::is_valid_budget(parts[3])
}

fn catalog_stale_tier_model_reason(path: &str, value: &str) -> Option<String> {
    if !is_tier_model_spec_value_path(path) {
        return None;
    }
    let parts: Vec<&str> = value.split('/').collect();
    if parts.len() != 4 || !is_known_active_tool(parts[0]) {
        return None;
    }
    if !is_known_provider(parts[0], parts[1]) {
        return Some(format!(
            "unknown provider '{}' for tool '{}'",
            parts[1], parts[0]
        ));
    }
    if !is_known_model(parts[0], parts[1], parts[2]) {
        return Some(format!(
            "unknown model '{}' for tool '{}' provider '{}'",
            parts[2], parts[0], parts[1]
        ));
    }
    None
}

fn is_known_active_tool(tool: &str) -> bool {
    !csa_core::types::is_removed_tool_name(tool)
        && crate::global::all_known_tools()
            .iter()
            .any(|known| known.as_str() == tool)
}

fn is_known_provider(tool: &str, provider: &str) -> bool {
    !csa_core::model_catalog::provider_validation_enabled(tool)
        || csa_core::model_catalog::valid_providers(tool).contains(&provider)
}

fn is_known_model(tool: &str, provider: &str, model: &str) -> bool {
    !csa_core::model_catalog::model_validation_enabled(tool)
        || csa_core::model_catalog::valid_models(tool, provider).contains(&model)
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

fn is_tier_models_array_path(path: &str) -> bool {
    path.starts_with("$.tiers.") && path.ends_with(".models")
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

fn warn_stale_model(source: &str, path: &str, value: &str, reason: &str) {
    eprintln!(
        "warning: project config {source}: ignoring stale tier model spec '{value}' at {path}: {reason}. Action: remove this entry or replace it with a current codex/claude-code spec."
    );
}
