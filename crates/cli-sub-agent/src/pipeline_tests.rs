use super::*;
use chrono::Utc;
use csa_config::config::CURRENT_SCHEMA_VERSION;
use csa_config::{ProjectMeta, ResourcesConfig};
use std::collections::HashMap;

#[test]
fn determine_project_root_none_returns_cwd() {
    let result = determine_project_root(None).unwrap();
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    assert_eq!(result, cwd);
}

#[test]
fn determine_project_root_with_valid_path() {
    let tmp = tempfile::tempdir().unwrap();
    let result = determine_project_root(Some(tmp.path().to_str().unwrap())).unwrap();
    assert_eq!(result, tmp.path().canonicalize().unwrap());
}

#[test]
fn determine_project_root_nonexistent_path_errors() {
    let result = determine_project_root(Some("/nonexistent/path/12345"));
    assert!(result.is_err());
}

#[test]
fn load_and_validate_exceeds_depth_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    // With no config, max_depth defaults to 5
    let result = load_and_validate(tmp.path(), 100).unwrap();
    assert!(
        result.is_none(),
        "Should return None when depth exceeds max"
    );
}

#[test]
fn load_and_validate_within_depth_returns_some() {
    let tmp = tempfile::tempdir().unwrap();
    let result = load_and_validate(tmp.path(), 0).unwrap();
    assert!(
        result.is_some(),
        "Should return Some when depth is within bounds"
    );
}

#[test]
fn resolve_idle_timeout_prefers_cli_override() {
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            min_free_memory_mb: 4096,
            idle_timeout_seconds: 111,
            initial_estimates: HashMap::new(),
        },
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    assert_eq!(resolve_idle_timeout_seconds(Some(&cfg), Some(42)), 42);
}

#[test]
fn resolve_idle_timeout_uses_config_then_default() {
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            min_free_memory_mb: 4096,
            idle_timeout_seconds: 222,
            initial_estimates: HashMap::new(),
        },
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    assert_eq!(resolve_idle_timeout_seconds(Some(&cfg), None), 222);
    assert_eq!(
        resolve_idle_timeout_seconds(None, None),
        DEFAULT_IDLE_TIMEOUT_SECONDS
    );
}
