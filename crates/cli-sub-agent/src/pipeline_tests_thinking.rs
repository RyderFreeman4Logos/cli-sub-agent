use super::*;
use csa_config::config::{CURRENT_SCHEMA_VERSION, TierStrategy, ToolConfig};
use csa_config::global::GlobalToolConfig;
use csa_config::{ProjectMeta, ResourcesConfig};
use std::collections::HashMap;

fn config_with_single_tier_model(
    tier_name: &str,
    tool_name: &str,
    model_spec: &str,
) -> ProjectConfig {
    let mut tools = HashMap::new();
    tools.insert(
        tool_name.to_string(),
        ToolConfig {
            enabled: true,
            ..Default::default()
        },
    );

    let mut tiers = HashMap::new();
    tiers.insert(
        tier_name.to_string(),
        csa_config::config::TierConfig {
            description: "test".to_string(),
            models: vec![model_spec.to_string()],
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers,
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
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    }
}

fn global_openai_compat_env_config() -> GlobalConfig {
    let mut global_config = GlobalConfig::default();
    global_config.tools.insert(
        "openai-compat".to_string(),
        GlobalToolConfig {
            env: HashMap::from([
                (
                    "OPENAI_COMPAT_BASE_URL".to_string(),
                    "http://localhost:8317".to_string(),
                ),
                ("OPENAI_COMPAT_API_KEY".to_string(), "test-key".to_string()),
                ("OPENAI_COMPAT_MODEL".to_string(), "local-model".to_string()),
            ]),
            ..Default::default()
        },
    );
    global_config
}

#[tokio::test]
async fn build_and_validate_executor_accepts_global_openai_compat_env() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .lock_owned()
        .await;
    let _base = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_BASE_URL");
    let _key = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_API_KEY");
    let _model = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_MODEL");
    let cfg = config_with_single_tier_model(
        "tier-3-complex",
        "openai-compat",
        "openai-compat/openai/gpt-5/high",
    );
    let global_config = global_openai_compat_env_config();

    let result = build_and_validate_executor(
        &ToolName::OpenaiCompat,
        Some("openai-compat/openai/gpt-5/high"),
        None,
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: Some(&global_config),
        },
        true,
        false,
        false,
    )
    .await;

    assert!(
        result.is_ok(),
        "global openai-compat env should satisfy pre-spawn availability: {result:?}"
    );
}

#[tokio::test]
async fn openai_compat_model_spec_overrides_project_default_model() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .lock_owned()
        .await;
    let _base = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_BASE_URL");
    let _key = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_API_KEY");
    let _model = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_MODEL");

    let mut cfg = config_with_single_tier_model(
        "tier-3-complex",
        "openai-compat",
        "openai-compat/openai/gpt-5/high",
    );
    let tool = cfg
        .tools
        .get_mut("openai-compat")
        .expect("openai-compat test tool config exists");
    tool.base_url = Some("http://localhost:8317".to_string());
    tool.api_key = Some("test-key".to_string());
    tool.default_model = Some("local-default".to_string());

    let exec = build_and_validate_executor(
        &ToolName::OpenaiCompat,
        Some("openai-compat/openai/gpt-5/high"),
        None,
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        true,
        false,
        true,
    )
    .await
    .expect("project HTTP config plus explicit model spec should be valid");

    match exec {
        csa_executor::Executor::OpenaiCompat { model_override, .. } => {
            assert_eq!(model_override.as_deref(), Some("gpt-5"));
        }
        other => panic!("expected openai-compat executor, got {other:?}"),
    }
}

/// When project config has `thinking_lock` for a tool, the CLI `--thinking`
/// value must be overridden. Verify via Executor's Debug representation.
#[tokio::test]
async fn thinking_lock_project_config_overrides_cli_thinking() {
    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            thinking_lock: Some("xhigh".to_string()),
            ..Default::default()
        },
    );
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    let result = build_and_validate_executor(
        &ToolName::Codex,
        None,
        None,
        Some("low"), // CLI says low, but lock says xhigh
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        false,
        false,
        false,
    )
    .await;

    // If tool is installed, verify thinking is locked to Xhigh.
    // If not installed, that's OK — the lock resolution happens before install check.
    if let Ok(exec) = result {
        let debug = format!("{exec:?}");
        assert!(
            debug.contains("Xhigh"),
            "thinking_lock should override CLI --thinking to Xhigh, got: {debug}"
        );
    }
}

/// When global config has `thinking_lock`, it should apply when project config
/// does not have one.
#[tokio::test]
async fn thinking_lock_global_config_applies_when_project_absent() {
    let mut global_tools = HashMap::new();
    global_tools.insert(
        "codex".to_string(),
        GlobalToolConfig {
            thinking_lock: Some("high".to_string()),
            ..Default::default()
        },
    );
    let global_cfg = csa_config::GlobalConfig {
        tools: global_tools,
        ..Default::default()
    };

    let result = build_and_validate_executor(
        &ToolName::Codex,
        None,
        None,
        Some("low"), // CLI says low, but global lock says high
        ConfigRefs {
            project: None,
            global: Some(&global_cfg),
        },
        false,
        false,
        false,
    )
    .await;

    if let Ok(exec) = result {
        let debug = format!("{exec:?}");
        assert!(
            debug.contains("High"),
            "global thinking_lock should override CLI --thinking to High, got: {debug}"
        );
    }
}

/// Project config `thinking_lock` takes precedence over global config.
#[tokio::test]
async fn thinking_lock_project_overrides_global() {
    let mut project_tools = HashMap::new();
    project_tools.insert(
        "codex".to_string(),
        ToolConfig {
            thinking_lock: Some("xhigh".to_string()),
            ..Default::default()
        },
    );
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: project_tools,
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
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    let mut global_tools = HashMap::new();
    global_tools.insert(
        "codex".to_string(),
        GlobalToolConfig {
            thinking_lock: Some("low".to_string()), // global says low
            ..Default::default()
        },
    );
    let global_cfg = csa_config::GlobalConfig {
        tools: global_tools,
        ..Default::default()
    };

    let result = build_and_validate_executor(
        &ToolName::Codex,
        None,
        None,
        None, // no CLI thinking
        ConfigRefs {
            project: Some(&cfg),
            global: Some(&global_cfg),
        },
        false,
        false,
        false,
    )
    .await;

    if let Ok(exec) = result {
        let debug = format!("{exec:?}");
        assert!(
            debug.contains("Xhigh"),
            "project thinking_lock (xhigh) must override global (low), got: {debug}"
        );
    }
}

/// When no thinking_lock is configured, CLI `--thinking` should pass through.
#[tokio::test]
async fn no_thinking_lock_passes_cli_thinking_through() {
    let result = build_and_validate_executor(
        &ToolName::Codex,
        None,
        None,
        Some("medium"), // CLI medium, no lock
        ConfigRefs {
            project: None,
            global: None,
        },
        false,
        false,
        false,
    )
    .await;

    if let Ok(exec) = result {
        let debug = format!("{exec:?}");
        assert!(
            debug.contains("Medium"),
            "without thinking_lock, CLI --thinking should pass through, got: {debug}"
        );
    }
}

#[tokio::test]
async fn model_thinking_suffix_is_stripped_before_tier_validation() {
    let cfg = config_with_single_tier_model(
        "tier-4-critical",
        "gemini-cli",
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
    );

    let result = build_and_validate_executor(
        &ToolName::GeminiCli,
        None,
        Some("google/gemini-3.1-pro-preview/xhigh"),
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        true,
        false,
        false,
    )
    .await;

    if let Err(err) = result {
        let msg = err.to_string();
        assert!(
            !msg.contains("not configured in any tier"),
            "thinking suffix should be stripped before tier validation: {msg}"
        );
    }
}

#[tokio::test]
async fn force_ignore_tier_setting_skips_execution_boundary_model_check() {
    let cfg = config_with_single_tier_model(
        "tier-4-critical",
        "gemini-cli",
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
    );

    let result = build_and_validate_executor(
        &ToolName::GeminiCli,
        None,
        Some("google/gemini-2.5-pro/xhigh"),
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        false, // `--force-ignore-tier-setting` disables defense-in-depth tier enforcement
        false,
        false,
    )
    .await;

    if let Err(err) = result {
        let msg = err.to_string();
        assert!(
            !msg.contains("not configured in any tier"),
            "force-ignore-tier-setting should bypass execution-boundary tier validation: {msg}"
        );
    }
}

#[tokio::test]
async fn project_default_thinking_applies_when_cli_absent() {
    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            default_thinking: Some("xhigh".to_string()),
            ..Default::default()
        },
    );
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    let result = build_and_validate_executor(
        &ToolName::Codex,
        None,
        None,
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        false,
        false,
        true,
    )
    .await;

    if let Ok(exec) = result {
        let debug = format!("{exec:?}");
        assert!(
            debug.contains("Xhigh"),
            "project default_thinking should apply when CLI omits --thinking, got: {debug}"
        );
    }
}

#[tokio::test]
async fn project_default_model_is_checked_against_tiers_when_enabled() {
    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            default_model: Some("gpt-4o".to_string()),
            ..Default::default()
        },
    );
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-2-standard".to_string(),
        csa_config::config::TierConfig {
            description: "test".to_string(),
            models: vec!["codex/openai/gpt-5.4/xhigh".to_string()],
            strategy: TierStrategy::default(),

            token_budget: None,
            max_turns: None,
        },
    );
    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("default".to_string(), "tier-2-standard".to_string());
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers,
        tier_mapping,
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
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    let result = build_and_validate_executor(
        &ToolName::Codex,
        None,
        None,
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        true,
        false,
        true,
    )
    .await;

    let err = result.expect_err("tier validation should reject tool default_model");
    assert!(
        err.to_string().contains("not configured in any tier"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn project_default_model_is_ignored_when_tool_defaults_disabled() {
    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            default_model: Some("gpt-4o".to_string()),
            ..Default::default()
        },
    );
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-2-standard".to_string(),
        csa_config::config::TierConfig {
            description: "test".to_string(),
            models: vec!["codex/openai/gpt-5.4/xhigh".to_string()],
            strategy: TierStrategy::default(),

            token_budget: None,
            max_turns: None,
        },
    );
    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("default".to_string(), "tier-2-standard".to_string());
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers,
        tier_mapping,
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
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    let result = build_and_validate_executor(
        &ToolName::Codex,
        None,
        None,
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        true,
        false,
        false,
    )
    .await;

    if let Err(err) = result {
        let msg = err.to_string();
        assert!(
            !msg.contains("not configured in any tier"),
            "tool defaults disabled should not inject default_model into tier validation: {msg}"
        );
    }
}
