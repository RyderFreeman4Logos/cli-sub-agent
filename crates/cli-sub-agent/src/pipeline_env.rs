use std::collections::HashMap;

use csa_config::ProjectConfig;

fn next_depth_value() -> String {
    let current_depth = std::env::var("CSA_DEPTH")
        .ok()
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or(0);
    current_depth.saturating_add(1).to_string()
}

pub(crate) fn build_merged_env(
    extra_env: Option<&HashMap<String, String>>,
    config: Option<&ProjectConfig>,
    tool_name: &str,
) -> HashMap<String, String> {
    let suppress = config
        .map(|c| c.should_suppress_notify(tool_name))
        .unwrap_or(true);

    let mut merged_env = extra_env.cloned().unwrap_or_default();
    if suppress {
        merged_env.insert("CSA_SUPPRESS_NOTIFY".to_string(), "1".to_string());
    }

    if let Some(limit_mb) = config.and_then(|c| c.sandbox_node_heap_limit_mb(tool_name)) {
        let heap_flag = format!("--max-old-space-size={limit_mb}");
        merged_env
            .entry("NODE_OPTIONS".to_string())
            .and_modify(|value| {
                if value.is_empty() {
                    *value = heap_flag.clone();
                } else {
                    value.push(' ');
                    value.push_str(&heap_flag);
                }
            })
            .or_insert(heap_flag);
    }

    merged_env.insert("CSA_DEPTH".to_string(), next_depth_value());
    merged_env.insert("CSA_INTERNAL_INVOCATION".to_string(), "1".to_string());

    merged_env
}
