use std::collections::HashMap;
use std::path::Path;

use crate::cli::{Cli, Commands, ReviewArgs, validate_review_args};
use crate::startup_env::StartupSubtreeEnv;
use crate::test_env_lock::ScopedEnvVarRestore;
use crate::test_session_sandbox::ScopedSessionSandbox;
use clap::Parser;
use csa_config::{ProjectConfig, ProjectMeta, ResourcesConfig, TierStrategy, ToolConfig};
use csa_core::types::ToolName;

fn project_config_with_quality_tier() -> ProjectConfig {
    let mut tools = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        tools.insert(
            tool.as_str().to_string(),
            ToolConfig {
                enabled: false,
                suppress_notify: true,
                ..Default::default()
            },
        );
    }
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: true,
            suppress_notify: true,
            ..Default::default()
        },
    );

    let mut tiers = HashMap::new();
    tiers.insert(
        "quality".to_string(),
        csa_config::config::TierConfig {
            description: "Test quality tier".to_string(),
            models: vec!["codex/openai/gpt-5.5/xhigh".to_string()],
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta::default(),
        resources: ResourcesConfig {
            min_free_memory_mb: 1,
            ..Default::default()
        },
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

fn write_project_config(project_root: &Path, config: &ProjectConfig) {
    let config_path = ProjectConfig::config_path(project_root);
    std::fs::create_dir_all(config_path.parent().expect("config parent")).unwrap();
    std::fs::write(config_path, toml::to_string_pretty(config).unwrap()).unwrap();
}

fn parse_review_args(project_root: &Path, extra: &[&str]) -> ReviewArgs {
    let cd = project_root.display().to_string();
    let mut argv = vec!["csa", "review", "--cd", cd.as_str(), "--diff"];
    argv.extend_from_slice(extra);
    let cli = Cli::try_parse_from(argv).expect("review CLI args should parse");
    match cli.command {
        Commands::Review(args) => {
            validate_review_args(&args).expect("review CLI args should validate");
            args
        }
        _ => panic!("expected review subcommand"),
    }
}

#[test]
fn review_invalid_tier_is_rejected_before_session_creation() {
    let project_dir = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let _config_home =
        ScopedEnvVarRestore::set("XDG_CONFIG_HOME", project_dir.path().join("xdg-config"));
    write_project_config(project_dir.path(), &project_config_with_quality_tier());
    let args = parse_review_args(project_dir.path(), &["--tier", "missing-tier"]);

    let err = super::validate_before_session(&args, &StartupSubtreeEnv::default())
        .expect_err("invalid tier must fail before review execution creates a session");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("Tier selector 'missing-tier' not found"),
        "{msg}"
    );
    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert!(
        sessions.is_empty(),
        "pre-session validation must not create a review session"
    );
}

#[test]
fn review_host_memory_admission_is_rejected_before_session_creation() {
    let project_dir = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let _config_home =
        ScopedEnvVarRestore::set("XDG_CONFIG_HOME", project_dir.path().join("xdg-config"));
    let _tools_available =
        ScopedEnvVarRestore::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let mut config = project_config_with_quality_tier();
    config.resources.min_free_memory_mb = u64::MAX / 2;
    write_project_config(project_dir.path(), &config);
    let args = parse_review_args(
        project_dir.path(),
        &["--tier", "quality", "--tool", "codex"],
    );

    let err = super::validate_before_session(&args, &StartupSubtreeEnv::default())
        .expect_err("impossible host-memory admission must fail before session creation");

    let msg = format!("{err:#}");
    assert!(msg.contains("CSA: low memory"), "{msg}");
    assert!(msg.contains("review preflight for tool 'codex'"), "{msg}");
    assert!(msg.contains("host memory retry guidance"), "{msg}");
    assert!(msg.contains("Retry feasibility:"), "{msg}");
    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert!(
        sessions.is_empty(),
        "pre-session memory validation must not create a review session"
    );
}

#[test]
fn fix_finding_prompt_file_dash_reaches_fix_finding_validation() {
    let project_dir = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let _config_home =
        ScopedEnvVarRestore::set("XDG_CONFIG_HOME", project_dir.path().join("xdg-config"));
    write_project_config(project_dir.path(), &project_config_with_quality_tier());
    let args = parse_review_args(
        project_dir.path(),
        &[
            "--fix-finding",
            "--session",
            "01KW54Y11RVVPXB5AT8STHDYC4",
            "--prompt-file",
            "-",
        ],
    );

    let err = super::validate_before_session(&args, &StartupSubtreeEnv::default())
        .expect_err("fix-finding route validation should run before stdin prompt resolution");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("failed to resolve --session 01KW54Y11RVVPXB5AT8STHDYC4 for --fix-finding"),
        "{msg}"
    );
    assert!(
        !msg.contains("review prompt file"),
        "fix-finding stdin sentinel must not use regular review prompt-file validation: {msg}"
    );
    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert!(
        sessions.is_empty(),
        "pre-session fix-finding validation must not create a session"
    );
}

#[test]
fn explicit_tool_preflight_validates_only_primary_candidate() {
    let candidates = vec![
        (
            ToolName::Opencode,
            Some("opencode/openai/gpt-5/xhigh".to_string()),
        ),
        (
            ToolName::Codex,
            Some("codex/openai/gpt-5.5/xhigh".to_string()),
        ),
    ];

    assert_eq!(
        super::pre_session_candidate_tools_to_validate(&candidates, true),
        vec![ToolName::Opencode],
        "explicit --tool with tier failover must preflight only the requested primary reviewer"
    );
}

#[test]
fn single_candidate_preflight_still_validates_candidate() {
    let candidates = vec![(
        ToolName::Codex,
        Some("codex/openai/gpt-5.5/xhigh".to_string()),
    )];

    assert_eq!(
        super::pre_session_candidate_tools_to_validate(&candidates, false),
        vec![ToolName::Codex],
        "single resolved reviewer remains safe to reject before session creation"
    );
}

#[test]
fn auto_tier_multi_candidate_preflight_defers_until_runtime_attempt() {
    let candidates = vec![
        (
            ToolName::Opencode,
            Some("opencode/openai/gpt-5/xhigh".to_string()),
        ),
        (
            ToolName::Codex,
            Some("codex/openai/gpt-5.5/xhigh".to_string()),
        ),
    ];

    assert!(
        super::pre_session_candidate_tools_to_validate(&candidates, false).is_empty(),
        "multi-candidate auto tiers must not let an unused fallback candidate block pre-session"
    );
}
