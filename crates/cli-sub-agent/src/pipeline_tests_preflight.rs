use super::*;
use std::collections::HashMap;

use csa_config::config::CURRENT_SCHEMA_VERSION;
use csa_config::{ProjectMeta, ResourcesConfig};

use crate::test_session_sandbox::ScopedSessionSandbox;

#[tokio::test]
async fn execute_with_session_and_meta_fails_preflight_before_creating_session() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    std::fs::write(temp.path().join("AGENTS.md"), "not a symlink").unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
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
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: csa_config::PreflightConfig {
            ai_config_symlink_check: csa_config::AiConfigSymlinkCheckConfig {
                enabled: true,
                ..Default::default()
            },
        },
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };
    let executor = Executor::Opencode {
        model_override: None,
        agent: None,
        thinking_budget: None,
    };

    let err = match execute_with_session_and_meta(
        &executor,
        &ToolName::Opencode,
        "review prompt",
        csa_core::types::OutputFormat::Json,
        None,
        false,
        Some("preflight-fail".to_string()),
        None,
        temp.path(),
        Some(&config),
        None,
        Some("run"),
        None,
        None,
        csa_process::StreamMode::BufferOnly,
        DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        None,
        None,
        None,
        None,
        false,
        false,
        &[],
        &[],
    )
    .await
    {
        Ok(_) => panic!("preflight should fail"),
        Err(err) => err,
    };

    let message = err.to_string();
    assert!(message.contains("preflight: AI-config symlink integrity check failed"));
    assert!(message.contains("AGENTS.md"));

    let sessions = csa_session::list_sessions(temp.path(), None).unwrap();
    assert!(
        sessions.is_empty(),
        "preflight failure must not create session"
    );
}

#[tokio::test]
async fn execute_with_session_and_meta_runs_preflight_for_fresh_spawn_override() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    std::fs::write(temp.path().join("AGENTS.md"), "not a symlink").unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
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
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: csa_config::PreflightConfig {
            ai_config_symlink_check: csa_config::AiConfigSymlinkCheckConfig {
                enabled: true,
                ..Default::default()
            },
        },
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };
    let executor = Executor::Opencode {
        model_override: None,
        agent: None,
        thinking_budget: None,
    };

    let err = match execute_with_session_and_meta(
        &executor,
        &ToolName::Opencode,
        "review prompt",
        csa_core::types::OutputFormat::Json,
        Some("01K00000000000000000000000".to_string()),
        true,
        Some("fresh-fork-spawn".to_string()),
        None,
        temp.path(),
        Some(&config),
        None,
        Some("run"),
        None,
        None,
        csa_process::StreamMode::BufferOnly,
        DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        None,
        None,
        None,
        None,
        false,
        false,
        &[],
        &[],
    )
    .await
    {
        Ok(_) => panic!("fresh spawn override should force preflight"),
        Err(err) => err,
    };

    let message = err.to_string();
    assert!(message.contains("preflight: AI-config symlink integrity check failed"));
    assert!(message.contains("AGENTS.md"));
}

#[tokio::test]
async fn execute_with_session_and_meta_skips_preflight_for_resume_session() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    std::fs::write(temp.path().join("AGENTS.md"), "not a symlink").unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
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
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: csa_config::PreflightConfig {
            ai_config_symlink_check: csa_config::AiConfigSymlinkCheckConfig {
                enabled: true,
                ..Default::default()
            },
        },
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };
    let executor = Executor::Opencode {
        model_override: None,
        agent: None,
        thinking_budget: None,
    };

    let err = match execute_with_session_and_meta(
        &executor,
        &ToolName::Opencode,
        "review prompt",
        csa_core::types::OutputFormat::Json,
        Some("01K00000000000000000000000".to_string()),
        false,
        Some("resume-session".to_string()),
        None,
        temp.path(),
        Some(&config),
        None,
        Some("run"),
        None,
        None,
        csa_process::StreamMode::BufferOnly,
        DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        None,
        None,
        None,
        None,
        false,
        false,
        &[],
        &[],
    )
    .await
    {
        Ok(_) => panic!("resume should skip preflight and fail on missing session"),
        Err(err) => err,
    };

    let message = err.to_string();
    assert!(
        !message.contains("preflight: AI-config symlink integrity check failed"),
        "resume path should skip preflight, got: {message}"
    );
}
