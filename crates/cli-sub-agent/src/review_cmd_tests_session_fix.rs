use super::{
    SelectionResolutionCtx, effective_no_failover_for_session_fix,
    resolve_selection_or_persist_error, resolve_selection_tool, resolve_session_fix_selection,
    validate_session_fix_before_daemon,
};
use crate::cli::{Cli, Commands, ReviewArgs, validate_review_args};
use crate::test_env_lock::ScopedEnvVarRestore;
use crate::test_session_sandbox::ScopedSessionSandbox;
use clap::Parser;
use csa_config::{GlobalConfig, ProjectConfig, ProjectMeta, ResourcesConfig, ReviewConfig};
use csa_config::{TierStrategy, ToolConfig};
use csa_core::types::ToolName;
use std::collections::HashMap;
use std::path::Path;

fn project_config_with_enabled_tools(tools: &[&str]) -> ProjectConfig {
    let mut tool_map = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        tool_map.insert(
            tool.as_str().to_string(),
            ToolConfig {
                enabled: false,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
    }
    for tool in tools {
        tool_map.insert(
            (*tool).to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
    }

    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta::default(),
        resources: ResourcesConfig {
            min_free_memory_mb: 1,
            ..Default::default()
        },
        acp: Default::default(),
        tools: tool_map,
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

fn parse_review_args(argv: &[&str]) -> ReviewArgs {
    let cli = Cli::try_parse_from(argv).expect("review CLI args should parse");
    match cli.command {
        Commands::Review(args) => {
            validate_review_args(&args).expect("review CLI args should validate");
            args
        }
        _ => panic!("expected review subcommand"),
    }
}

fn write_review_project_config(project_root: &Path, config: &ProjectConfig) {
    let config_path = ProjectConfig::config_path(project_root);
    std::fs::create_dir_all(config_path.parent().expect("config parent")).unwrap();
    std::fs::write(config_path, toml::to_string_pretty(config).unwrap()).unwrap();
}

fn write_session_metadata(project_root: &Path, session_id: &str, tool: &str, tool_locked: bool) {
    let session_dir = csa_session::get_session_dir(project_root, session_id).unwrap();
    let metadata = csa_session::SessionMetadata {
        tool: tool.to_string(),
        tool_locked,
        runtime_binary: None,
    };
    std::fs::write(
        session_dir.join(csa_session::metadata::METADATA_FILE_NAME),
        toml::to_string_pretty(&metadata).unwrap(),
    )
    .unwrap();
}

fn write_session_result(project_root: &Path, session_id: &str, tool: &str) {
    let now = chrono::Utc::now();
    let result = csa_session::SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary: "failed review".to_string(),
        tool: tool.to_string(),
        started_at: now,
        completed_at: now,
        ..Default::default()
    };
    csa_session::save_result(project_root, session_id, &result).unwrap();
}

fn review_config_with_quality_tier() -> ProjectConfig {
    let mut config = project_config_with_enabled_tools(&["gemini-cli", "codex"]);
    config.tiers.insert(
        "quality".to_string(),
        csa_config::config::TierConfig {
            description: "Test quality tier".to_string(),
            models: vec![
                "gemini-cli/google/default/xhigh".to_string(),
                "codex/openai/gpt-5.5/xhigh".to_string(),
            ],
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    config.review = Some(ReviewConfig {
        tier: Some("quality".to_string()),
        ..Default::default()
    });
    config
}

fn review_config_with_gemini_only_quality_tier() -> ProjectConfig {
    let mut config = project_config_with_enabled_tools(&["gemini-cli", "codex"]);
    config.tiers.insert(
        "quality".to_string(),
        csa_config::config::TierConfig {
            description: "Test quality tier".to_string(),
            models: vec!["gemini-cli/google/default/xhigh".to_string()],
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    config.review = Some(ReviewConfig {
        tier: Some("quality".to_string()),
        ..Default::default()
    });
    config
}

fn parse_session_fix_args(project_root: &Path, session_id: &str, extra: &[&str]) -> ReviewArgs {
    let cd = project_root.display().to_string();
    let mut argv = vec![
        "csa",
        "review",
        "--cd",
        cd.as_str(),
        "--files",
        "src/lib.rs",
        "--session",
        session_id,
        "--fix",
    ];
    argv.extend_from_slice(extra);
    parse_review_args(&argv)
}

#[test]
fn review_session_fix_skips_non_concrete_metadata_and_uses_result_tool() {
    let project_dir = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let _tools_available =
        ScopedEnvVarRestore::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let _config_home =
        ScopedEnvVarRestore::set("XDG_CONFIG_HOME", project_dir.path().join("xdg-config"));
    let config = review_config_with_quality_tier();
    write_review_project_config(project_dir.path(), &config);
    let source = csa_session::create_session(project_dir.path(), Some("failed review"), None, None)
        .expect("source session should be created");
    write_session_metadata(
        project_dir.path(),
        &source.meta_session_id,
        "unknown",
        false,
    );
    write_session_result(project_dir.path(), &source.meta_session_id, "codex");
    let args = parse_session_fix_args(project_dir.path(), &source.meta_session_id, &[]);

    let resolved_tool = resolve_session_fix_selection(
        &args,
        project_dir.path(),
        Some(&config),
        &GlobalConfig::default(),
        Some("claude-code"),
    )
    .expect("session fix selection should use concrete result tool");

    assert_eq!(resolved_tool, Some(ToolName::Codex));
}

#[test]
fn review_without_session_fix_preserves_failover_setting() {
    assert!(!effective_no_failover_for_session_fix(false, None));
    assert!(effective_no_failover_for_session_fix(true, None));
}

#[test]
fn review_session_fix_suppresses_cross_tool_tier_candidates_after_selection() {
    let project_dir = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let _tools_available =
        ScopedEnvVarRestore::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let _config_home =
        ScopedEnvVarRestore::set("XDG_CONFIG_HOME", project_dir.path().join("xdg-config"));
    let config = review_config_with_quality_tier();
    let global_config = GlobalConfig::default();
    let source = csa_session::create_session(project_dir.path(), Some("failed review"), None, None)
        .expect("source session should be created");
    write_session_metadata(
        project_dir.path(),
        &source.meta_session_id,
        "unknown",
        false,
    );
    write_session_result(project_dir.path(), &source.meta_session_id, "codex");
    let args = parse_session_fix_args(project_dir.path(), &source.meta_session_id, &[]);
    let selection = resolve_selection_tool(&args, project_dir.path(), None)
        .expect("session fix selection tool should resolve");

    let resolved = resolve_selection_or_persist_error(SelectionResolutionCtx {
        args: &args,
        project_config: Some(&config),
        global_config: &global_config,
        parent_tool: Some("claude-code"),
        project_root: project_dir.path(),
        effective_tier: Some("quality"),
        selection_tool: selection.selection_tool,
        direct_tool_requested: selection.direct_tool_requested,
        session_fix: selection.session_fix.as_ref(),
        review_description: "review: src/lib.rs",
    })
    .expect("runtime selection should accept the recorded result tool");

    assert_eq!(resolved.tool, ToolName::Codex);
    assert_eq!(
        resolved.model_spec.as_deref(),
        Some("codex/openai/gpt-5.5/xhigh")
    );
    assert_eq!(resolved.tier_preference_order, vec!["codex".to_string()]);

    let execution_no_failover =
        effective_no_failover_for_session_fix(args.no_failover, selection.session_fix.as_ref());
    assert!(execution_no_failover);
    let tier_active = resolved.model_spec.is_some()
        && args.model_spec.is_none()
        && !args.force_ignore_tier_setting;
    assert!(tier_active);

    let candidates = crate::tier_model_fallback::ordered_tier_candidates(
        resolved.tool,
        resolved.model_spec.as_deref(),
        Some("quality"),
        Some(&config),
        Some(&global_config),
        tier_active && !execution_no_failover,
        &resolved.tier_preference_order,
    );

    assert_eq!(
        candidates,
        vec![(
            ToolName::Codex,
            Some("codex/openai/gpt-5.5/xhigh".to_string())
        )]
    );
}

#[test]
fn review_session_fix_rejects_tier_fallback_when_recorded_result_tool_missing_from_tier() {
    let project_dir = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let _tools_available =
        ScopedEnvVarRestore::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let _config_home =
        ScopedEnvVarRestore::set("XDG_CONFIG_HOME", project_dir.path().join("xdg-config"));
    let config = review_config_with_gemini_only_quality_tier();
    write_review_project_config(project_dir.path(), &config);
    let source = csa_session::create_session(project_dir.path(), Some("failed review"), None, None)
        .expect("source session should be created");
    write_session_metadata(
        project_dir.path(),
        &source.meta_session_id,
        "unknown",
        false,
    );
    write_session_result(project_dir.path(), &source.meta_session_id, "codex");
    let args = parse_session_fix_args(project_dir.path(), &source.meta_session_id, &[]);

    let err = resolve_session_fix_selection(
        &args,
        project_dir.path(),
        Some(&config),
        &GlobalConfig::default(),
        Some("claude-code"),
    )
    .expect_err("recorded tool missing from tier must not fall back to another tool");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("must use the original review tool 'codex'"),
        "unexpected error: {msg}"
    );
    assert!(
        msg.contains("tier/model routing resolved 'gemini-cli'"),
        "unexpected error: {msg}"
    );
    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "pre-daemon validation must not create a child session"
    );
}

#[test]
fn review_session_fix_runtime_resolution_rejects_tier_fallback_to_non_recorded_tool() {
    let project_dir = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let _tools_available =
        ScopedEnvVarRestore::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let _config_home =
        ScopedEnvVarRestore::set("XDG_CONFIG_HOME", project_dir.path().join("xdg-config"));
    let config = review_config_with_gemini_only_quality_tier();
    let global_config = GlobalConfig::default();
    let source = csa_session::create_session(project_dir.path(), Some("failed review"), None, None)
        .expect("source session should be created");
    write_session_metadata(
        project_dir.path(),
        &source.meta_session_id,
        "unknown",
        false,
    );
    write_session_result(project_dir.path(), &source.meta_session_id, "codex");
    let args = parse_session_fix_args(project_dir.path(), &source.meta_session_id, &[]);
    let selection = resolve_selection_tool(&args, project_dir.path(), None)
        .expect("session fix selection tool should resolve");

    let err = resolve_selection_or_persist_error(SelectionResolutionCtx {
        args: &args,
        project_config: Some(&config),
        global_config: &global_config,
        parent_tool: Some("claude-code"),
        project_root: project_dir.path(),
        effective_tier: Some("quality"),
        selection_tool: selection.selection_tool,
        direct_tool_requested: selection.direct_tool_requested,
        session_fix: selection.session_fix.as_ref(),
        review_description: "review: src/lib.rs",
    })
    .expect_err("runtime selection must enforce recorded session tool after tier routing");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("must use the original review tool 'codex'"),
        "unexpected error: {msg}"
    );
    assert!(
        msg.contains("tier/model routing resolved 'gemini-cli'"),
        "unexpected error: {msg}"
    );
}

#[test]
fn review_session_fix_selects_original_tool_without_direct_tool_restriction() {
    let project_dir = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let _tools_available =
        ScopedEnvVarRestore::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let _config_home =
        ScopedEnvVarRestore::set("XDG_CONFIG_HOME", project_dir.path().join("xdg-config"));
    let config = review_config_with_quality_tier();
    write_review_project_config(project_dir.path(), &config);
    let source = csa_session::create_session(
        project_dir.path(),
        Some("failed review"),
        None,
        Some("codex"),
    )
    .expect("source session should be created");
    let args = parse_session_fix_args(project_dir.path(), &source.meta_session_id, &[]);

    let resolved_tool = resolve_session_fix_selection(
        &args,
        project_dir.path(),
        Some(&config),
        &GlobalConfig::default(),
        Some("claude-code"),
    )
    .expect("session fix selection should resolve");

    assert_eq!(resolved_tool, Some(ToolName::Codex));
    let resume = csa_session::resolve_resume_session(
        project_dir.path(),
        &source.meta_session_id,
        ToolName::Codex.as_str(),
    )
    .expect("resolved tool must satisfy session lock");
    assert_eq!(resume.meta_session_id, source.meta_session_id);
}

#[test]
fn review_session_fix_without_recorded_tool_fails_before_child_session_creation() {
    let project_dir = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let _config_home =
        ScopedEnvVarRestore::set("XDG_CONFIG_HOME", project_dir.path().join("xdg-config"));
    let config = review_config_with_quality_tier();
    write_review_project_config(project_dir.path(), &config);
    let source =
        csa_session::create_session(project_dir.path(), Some("legacy failed review"), None, None)
            .expect("source session should be created");
    let args = parse_session_fix_args(project_dir.path(), &source.meta_session_id, &[]);

    let err = validate_session_fix_before_daemon(&args)
        .expect_err("missing recorded tool must fail before daemon spawn");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("Cannot infer review tool"),
        "unexpected error: {msg}"
    );
    assert!(
        msg.contains("metadata.toml tool, review_meta.json tool, or result.toml tool"),
        "unexpected error: {msg}"
    );
    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "pre-daemon validation must not create a child session"
    );
}

#[test]
fn review_session_fix_rejects_explicit_tool_mismatch_before_child_session_creation() {
    let project_dir = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let _config_home =
        ScopedEnvVarRestore::set("XDG_CONFIG_HOME", project_dir.path().join("xdg-config"));
    let config = review_config_with_quality_tier();
    write_review_project_config(project_dir.path(), &config);
    let source = csa_session::create_session(
        project_dir.path(),
        Some("failed review"),
        None,
        Some("codex"),
    )
    .expect("source session should be created");
    let args = parse_session_fix_args(
        project_dir.path(),
        &source.meta_session_id,
        &["--tool", "gemini-cli"],
    );

    let err = validate_session_fix_before_daemon(&args)
        .expect_err("explicit tool mismatch must fail before daemon spawn");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("must use the original review tool 'codex'"),
        "unexpected error: {msg}"
    );
    assert!(
        msg.contains("explicit --tool 'gemini-cli'"),
        "unexpected error: {msg}"
    );
    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "pre-daemon validation must not create a child session"
    );
}
