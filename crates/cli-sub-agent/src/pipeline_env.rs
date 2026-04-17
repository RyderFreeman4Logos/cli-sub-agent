use std::collections::HashMap;
use std::path::{Path, PathBuf};

use csa_config::ProjectConfig;
use tracing::info;

/// Read the current CSA recursion depth from the environment.
pub(crate) fn current_csa_depth() -> u32 {
    std::env::var("CSA_DEPTH")
        .ok()
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or(0)
}

fn next_depth_value() -> String {
    current_csa_depth().saturating_add(1).to_string()
}

/// Resolve effective cooldown seconds from config or default.
pub(crate) fn resolve_cooldown_seconds(config: Option<&ProjectConfig>) -> u64 {
    config
        .map(|c| c.session.cooldown_seconds)
        .unwrap_or(csa_config::DEFAULT_COOLDOWN_SECS)
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

pub(crate) fn apply_review_target_dir(
    task_type: Option<&str>,
    session_dir: &std::path::Path,
    merged_env: &mut HashMap<String, String>,
) {
    if matches!(task_type, Some("review")) {
        let project_root = resolve_review_project_root(session_dir)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let repo_target_dir = project_root.join("target");
        let user_configured_target = std::fs::symlink_metadata(&repo_target_dir)
            .map(|metadata| metadata.is_dir() || metadata.file_type().is_symlink())
            .unwrap_or(false);

        if user_configured_target {
            info!(
                project_target = %repo_target_dir.display(),
                "Review session: ./target already configured by user (detected symlink/dir), leaving CARGO_TARGET_DIR untouched"
            );
            return;
        }

        let review_target_dir = session_dir.join("target");
        info!(
            "Review session: no user-configured ./target, routing CARGO_TARGET_DIR to {}",
            review_target_dir.display()
        );
        merged_env.insert(
            "CARGO_TARGET_DIR".to_string(),
            review_target_dir.display().to_string(),
        );
    }
}

fn resolve_review_project_root(session_dir: &Path) -> Option<PathBuf> {
    let state_path = session_dir.join("state.toml");
    let state_contents = std::fs::read_to_string(state_path).ok()?;
    let state_value: toml::Value = toml::from_str(&state_contents).ok()?;
    let project_path = state_value.get("project_path")?.as_str()?;
    Some(PathBuf::from(project_path))
}
