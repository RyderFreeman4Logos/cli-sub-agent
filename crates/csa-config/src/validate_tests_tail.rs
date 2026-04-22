use super::*;
use crate::config::{
    CURRENT_SCHEMA_VERSION, ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, TierStrategy,
    ToolConfig,
};
use crate::global::ReviewConfig;
use crate::global::ToolSelection;
use chrono::Utc;
use std::collections::HashMap;
use tempfile::tempdir;

#[test]
fn test_validate_model_spec_two_parts() {
    let dir = tempdir().unwrap();

    let mut tiers = HashMap::new();
    tiers.insert(
        "bad-tier".to_string(),
        TierConfig {
            description: "Bad".to_string(),
            models: vec!["tool/model".to_string()],
            strategy: TierStrategy::default(),

            token_budget: None,
            max_turns: None,
        },
    );

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    config.save(dir.path()).unwrap();
    let config_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &config_path);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("invalid model spec")
    );
}

#[test]
fn test_validate_model_spec_five_parts() {
    let dir = tempdir().unwrap();

    let mut tiers = HashMap::new();
    tiers.insert(
        "bad-tier".to_string(),
        TierConfig {
            description: "Bad".to_string(),
            models: vec!["a/b/c/d/e".to_string()],
            strategy: TierStrategy::default(),

            token_budget: None,
            max_turns: None,
        },
    );

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    config.save(dir.path()).unwrap();
    let config_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &config_path);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("invalid model spec")
    );
}

#[test]
fn test_validate_review_tool_auto_accepted() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: Some(ReviewConfig {
            tool: ToolSelection::Single("auto".to_string()),
            ..Default::default()
        }),
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    config.save(dir.path()).unwrap();
    let config_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &config_path);
    assert!(result.is_ok(), "'auto' should be a valid review tool");
}

#[test]
fn test_validate_review_batch_commits_zero_rejected() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: Some(ReviewConfig {
            batch_commits: 0,
            ..Default::default()
        }),
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    config.save(dir.path()).unwrap();
    let config_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &config_path);
    let err = result.expect_err("batch_commits=0 should be rejected");
    assert!(
        err.to_string()
            .contains("[review].batch_commits must be at least 1"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_validate_all_known_review_tools_accepted() {
    let known = ["auto", "gemini-cli", "opencode", "codex", "claude-code"];
    for tool_name in &known {
        let dir = tempdir().unwrap();

        let config = ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            acp: Default::default(),
            tools: HashMap::new(),
            review: Some(ReviewConfig {
                tool: ToolSelection::Single(tool_name.to_string()),
                ..Default::default()
            }),
            debate: None,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
            tool_aliases: HashMap::new(),
            preferences: None,
            session: Default::default(),
            memory: Default::default(),
            hooks: Default::default(),
            execution: Default::default(),
            session_wait: None,
            preflight: Default::default(),
            vcs: Default::default(),
            filesystem_sandbox: Default::default(),
        };

        config.save(dir.path()).unwrap();
        let config_path = dir.path().join(".csa").join("config.toml");
        let result = validate_config_with_paths(None, &config_path);
        assert!(
            result.is_ok(),
            "Review tool '{tool_name}' should be accepted"
        );
    }
}

#[test]
fn test_validate_all_known_debate_tools_accepted() {
    let known = ["auto", "gemini-cli", "opencode", "codex", "claude-code"];
    for tool_name in &known {
        let dir = tempdir().unwrap();

        let config = ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            acp: Default::default(),
            tools: HashMap::new(),
            review: None,
            debate: Some(ReviewConfig {
                tool: ToolSelection::Single(tool_name.to_string()),
                ..Default::default()
            }),
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
            tool_aliases: HashMap::new(),
            preferences: None,
            session: Default::default(),
            memory: Default::default(),
            hooks: Default::default(),
            execution: Default::default(),
            session_wait: None,
            preflight: Default::default(),
            vcs: Default::default(),
            filesystem_sandbox: Default::default(),
        };

        config.save(dir.path()).unwrap();
        let config_path = dir.path().join(".csa").join("config.toml");
        let result = validate_config_with_paths(None, &config_path);
        assert!(
            result.is_ok(),
            "Debate tool '{tool_name}' should be accepted"
        );
    }
}

#[test]
fn test_validate_all_four_known_tools_accepted() {
    let dir = tempdir().unwrap();

    let mut tools = HashMap::new();
    for name in &["gemini-cli", "opencode", "codex", "claude-code"] {
        tools.insert(name.to_string(), ToolConfig::default());
    }

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    config.save(dir.path()).unwrap();
    let config_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &config_path);
    assert!(result.is_ok());
}

#[test]
fn test_validate_no_review_no_debate_is_ok() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    config.save(dir.path()).unwrap();
    let config_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &config_path);
    assert!(result.is_ok(), "No review/debate should be valid");
}

#[test]
fn test_validate_max_recursion_depth_zero() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 0,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    config.save(dir.path()).unwrap();
    let config_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &config_path);
    // 0 is <= 20, so should pass validation
    assert!(result.is_ok(), "max_recursion_depth 0 should be valid");
}

include!("validate_tests_deprecated.rs");
include!("validate_tests_preferences.rs");
include!("validate_tests_sandbox.rs");
include!("validate_tests_tiers.rs");
