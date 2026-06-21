use crate::pipeline_env::MergedEnvRequest;
use chrono::Utc;
use csa_config::config::CURRENT_SCHEMA_VERSION;
use csa_config::{ProjectConfig, ProjectMeta, ResourcesConfig};
use std::collections::HashMap;

fn test_config() -> ProjectConfig {
    ProjectConfig {
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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

#[test]
fn build_merged_env_propagates_pattern_internal_marker_to_leaf_tool() {
    let cfg = test_config();

    let marked = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: None,
        config: Some(&cfg),
        global_config: None,
        project_root: None,
        tool_name: "claude-code",
        current_depth: 0,
        pattern_internal: true,
        allow_git_push: false,
    });
    assert_eq!(
        marked
            .get(csa_core::env::CSA_PATTERN_INTERNAL_ENV_KEY)
            .map(String::as_str),
        Some("1")
    );

    let unmarked = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: None,
        config: Some(&cfg),
        global_config: None,
        project_root: None,
        tool_name: "claude-code",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });
    assert!(!unmarked.contains_key(csa_core::env::CSA_PATTERN_INTERNAL_ENV_KEY));
}

#[test]
fn build_merged_env_denies_git_push_for_sa_mode_without_explicit_authorization() {
    let cfg = test_config();
    let denied = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: None,
        config: Some(&cfg),
        global_config: None,
        project_root: None,
        tool_name: "claude-code",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });

    assert!(!denied.contains_key("CSA_GIT_PUSH_ALLOWED"));
}

#[test]
fn build_merged_env_allows_git_push_with_explicit_authorization() {
    let cfg = test_config();
    let allowed = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: None,
        config: Some(&cfg),
        global_config: None,
        project_root: None,
        tool_name: "claude-code",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: true,
    });

    assert_eq!(
        allowed.get("CSA_GIT_PUSH_ALLOWED").map(String::as_str),
        Some("true")
    );
    assert!(
        !allowed.contains_key(crate::pipeline_env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV),
        "internal authorization marker must not leak to tool env"
    );
}

#[test]
fn build_merged_env_removes_spoofed_git_push_permission_for_leaf_workers() {
    let cfg = test_config();
    let extra_env = HashMap::from([
        ("CSA_GIT_PUSH_ALLOWED".to_string(), "true".to_string()),
        (
            crate::pipeline_env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV.to_string(),
            "true".to_string(),
        ),
    ]);

    let leaf = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: Some(&extra_env),
        config: Some(&cfg),
        global_config: None,
        project_root: None,
        tool_name: "claude-code",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });

    assert!(!leaf.contains_key("CSA_GIT_PUSH_ALLOWED"));
    assert!(!leaf.contains_key(crate::pipeline_env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV));
}
