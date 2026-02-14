use super::*;
use crate::config::{
    CURRENT_SCHEMA_VERSION, ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, ToolConfig,
};
use crate::global::ReviewConfig;
use chrono::Utc;
use std::collections::HashMap;
use tempfile::tempdir;

#[test]
fn test_validate_config_succeeds_on_valid() {
    let dir = tempdir().unwrap();

    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: true,
            restrictions: None,
            suppress_notify: true,
        },
    );

    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-1-quick".to_string(),
        TierConfig {
            description: "Quick tasks".to_string(),
            models: vec!["gemini-cli/google/gemini-3-flash-preview/xhigh".to_string()],
            token_budget: None,
            max_turns: None,
        },
    );

    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("security_audit".to_string(), "tier-1-quick".to_string());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test-project".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools,
        review: None,
        debate: None,
        tiers,
        tier_mapping,
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();

    let result = validate_config(dir.path());
    assert!(result.is_ok());
}

#[test]
fn test_validate_config_fails_on_empty_name() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();

    let result = validate_config(dir.path());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("cannot be empty"));
}

#[test]
fn test_validate_config_fails_on_unknown_tool() {
    let dir = tempdir().unwrap();

    let mut tools = HashMap::new();
    tools.insert(
        "unknown-tool".to_string(),
        ToolConfig {
            enabled: true,
            restrictions: None,
            suppress_notify: true,
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
        tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();

    let result = validate_config(dir.path());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Unknown tool"));
}

#[test]
fn test_validate_config_fails_on_invalid_review_tool() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        review: Some(ReviewConfig {
            tool: "invalid-tool".to_string(),
        }),
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();

    let result = validate_config(dir.path());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Invalid [review].tool value")
    );
}

#[test]
fn test_validate_config_fails_on_invalid_model_spec() {
    let dir = tempdir().unwrap();

    let mut tiers = HashMap::new();
    tiers.insert(
        "test-tier".to_string(),
        TierConfig {
            description: "Test tier".to_string(),
            models: vec!["invalid-model-spec".to_string()],
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
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();

    let result = validate_config(dir.path());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("invalid model spec")
    );
}

#[test]
fn test_validate_config_fails_on_invalid_tier_mapping() {
    let dir = tempdir().unwrap();

    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-1-quick".to_string(),
        TierConfig {
            description: "Quick tasks".to_string(),
            models: vec!["gemini-cli/google/gemini-3-flash-preview/xhigh".to_string()],
            token_budget: None,
            max_turns: None,
        },
    );

    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("security_audit".to_string(), "nonexistent-tier".to_string());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping,
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();

    let result = validate_config(dir.path());
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("tier_mapping") && err_msg.contains("unknown tier"));
}

#[test]
fn test_validate_config_fails_if_no_config() {
    let dir = tempdir().unwrap();
    // Use validate_config_with_paths(None, ...) to bypass user-level
    // fallback AND exercise the full validation path (None -> bail!).
    let project_path = dir.path().join(".csa").join("config.toml");
    let result = validate_config_with_paths(None, &project_path);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("No configuration found")
    );
}

#[test]
fn test_validate_config_fails_on_empty_models() {
    let dir = tempdir().unwrap();

    let mut tiers = HashMap::new();
    tiers.insert(
        "empty-tier".to_string(),
        TierConfig {
            description: "Empty tier".to_string(),
            models: vec![],
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
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();

    let result = validate_config(dir.path());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("must have at least one model")
    );
}

#[test]
fn test_validate_config_accepts_custom_tier_names() {
    let dir = tempdir().unwrap();

    let mut tiers = HashMap::new();
    tiers.insert(
        "my-custom-tier".to_string(),
        TierConfig {
            description: "Custom tier name".to_string(),
            models: vec!["gemini-cli/google/gemini-3-flash-preview/xhigh".to_string()],
            token_budget: None,
            max_turns: None,
        },
    );

    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("analysis".to_string(), "my-custom-tier".to_string());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping,
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();

    let result = validate_config(dir.path());
    assert!(result.is_ok());
}

#[test]
fn test_validate_config_fails_on_invalid_debate_tool() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        review: None,
        debate: Some(ReviewConfig {
            tool: "invalid-tool".to_string(),
        }),
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();

    let result = validate_config(dir.path());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Invalid [debate].tool value")
    );
}

#[test]
fn test_validate_max_recursion_depth_boundary_20() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 20, // exactly at boundary
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
    assert!(result.is_ok(), "max_recursion_depth 20 should be valid");
}

#[test]
fn test_validate_max_recursion_depth_boundary_21() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 21, // just above boundary
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("too high"));
}

#[test]
fn test_validate_model_spec_two_parts() {
    let dir = tempdir().unwrap();

    let mut tiers = HashMap::new();
    tiers.insert(
        "bad-tier".to_string(),
        TierConfig {
            description: "Bad".to_string(),
            models: vec!["tool/model".to_string()],
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
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
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
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
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
        tools: HashMap::new(),
        review: Some(ReviewConfig {
            tool: "auto".to_string(),
        }),
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
    assert!(result.is_ok(), "'auto' should be a valid review tool");
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
            tools: HashMap::new(),
            review: Some(ReviewConfig {
                tool: tool_name.to_string(),
            }),
            debate: None,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();
        let result = validate_config(dir.path());
        assert!(
            result.is_ok(),
            "Review tool '{}' should be accepted",
            tool_name
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
            tools: HashMap::new(),
            review: None,
            debate: Some(ReviewConfig {
                tool: tool_name.to_string(),
            }),
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        config.save(dir.path()).unwrap();
        let result = validate_config(dir.path());
        assert!(
            result.is_ok(),
            "Debate tool '{}' should be accepted",
            tool_name
        );
    }
}

#[test]
fn test_validate_all_four_known_tools_accepted() {
    let dir = tempdir().unwrap();

    let mut tools = HashMap::new();
    for name in &["gemini-cli", "opencode", "codex", "claude-code"] {
        tools.insert(
            name.to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
                suppress_notify: true,
            },
        );
    }

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
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
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
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
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
    };

    config.save(dir.path()).unwrap();
    let result = validate_config(dir.path());
    // 0 is <= 20, so should pass validation
    assert!(result.is_ok(), "max_recursion_depth 0 should be valid");
}

include!("validate_tests_tiers.rs");
