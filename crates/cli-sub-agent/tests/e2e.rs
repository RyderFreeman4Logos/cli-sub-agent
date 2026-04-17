// End-to-end tests for the csa binary.
// Requires actual tool installations for full testing.

#[path = "../src/cli.rs"]
mod cli_defs;

use clap::Parser;
use cli_defs::{AuditCommands, Cli, Commands, McpHubCommands, validate_command_args};
use csa_core::types::OutputFormat;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Create a [`Command`] pointing at the built `csa` binary with HOME, XDG_STATE_HOME,
/// and XDG_CONFIG_HOME redirected to the given temp directory so tests never touch
/// real user state.
fn csa_cmd(tmp: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"));
    cmd
}

fn global_config_path(tmp: &Path) -> std::path::PathBuf {
    // Mirror the production resolver so the test writes the same platform-specific
    // global path that `csa config get` reads on Linux and macOS.
    if cfg!(target_os = "macos") {
        tmp.join("Library/Application Support/cli-sub-agent/config.toml")
    } else {
        tmp.join(".config/cli-sub-agent/config.toml")
    }
}

/// Run `csa init` (minimal mode) inside the given temp directory.
fn init_project(tmp: &std::path::Path) {
    let status = csa_cmd(tmp)
        .arg("init")
        .current_dir(tmp)
        .status()
        .expect("failed to run csa init");
    assert!(status.success(), "csa init should succeed");
}

/// Run `csa init --full` inside the given temp directory (full auto-detection mode).
fn init_project_full(tmp: &std::path::Path) {
    let status = csa_cmd(tmp)
        .args(["init", "--full"])
        .current_dir(tmp)
        .status()
        .expect("failed to run csa init --full");
    assert!(status.success(), "csa init --full should succeed");
}

fn write_project_config_with_tier(project_root: &Path) {
    let mut config = csa_config::ProjectConfig {
        schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
        project: csa_config::ProjectMeta {
            name: "test".to_string(),
            created_at: chrono::Utc::now(),
            max_recursion_depth: 5,
        },
        resources: csa_config::ResourcesConfig::default(),
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
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };
    config.tiers.insert(
        "default".to_string(),
        csa_config::config::TierConfig {
            description: "Test tier".to_string(),
            models: vec!["codex/gpt-5-codex/medium".to_string()],
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    let config_path = csa_config::ProjectConfig::config_path(project_root);
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        config_path,
        toml::to_string_pretty(&config).expect("serialize config"),
    )
    .expect("write config");
}

#[test]
fn cli_help_displays_correctly() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .arg("--help")
        .output()
        .expect("failed to run csa --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("CLI Sub-Agent"));
    assert!(stdout.contains("run"));
    assert!(stdout.contains("session"));
    assert!(stdout.contains("init"));
    assert!(stdout.contains("gc"));
    assert!(stdout.contains("config"));
}

#[test]
fn run_help_shows_tool_options() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["run", "--help"])
        .output()
        .expect("failed to run csa run --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--tool"));
    assert!(stdout.contains("--session"));
    assert!(stdout.contains("--ephemeral"));
    assert!(stdout.contains("--model"));
}

#[test]
fn mcp_hub_serve_parse_with_background_and_socket() {
    let cli = Cli::try_parse_from([
        "csa",
        "mcp-hub",
        "serve",
        "--background",
        "--socket",
        "/tmp/cli-sub-agent-1000/mcp-hub.sock",
    ])
    .expect("mcp-hub serve args should parse");

    match cli.command {
        Commands::McpHub {
            cmd:
                McpHubCommands::Serve {
                    background,
                    foreground,
                    socket,
                    http_bind,
                    http_port,
                    systemd_activation,
                },
        } => {
            assert!(background);
            assert!(!foreground);
            assert_eq!(
                socket.as_deref(),
                Some("/tmp/cli-sub-agent-1000/mcp-hub.sock")
            );
            assert!(http_bind.is_none());
            assert!(http_port.is_none());
            assert!(!systemd_activation);
        }
        _ => panic!("expected mcp-hub serve subcommand"),
    }
}

#[test]
fn mcp_hub_gen_skill_parse_with_socket() {
    let cli = Cli::try_parse_from([
        "csa",
        "mcp-hub",
        "gen-skill",
        "--socket",
        "/tmp/cli-sub-agent-1000/mcp-hub.sock",
    ])
    .expect("mcp-hub gen-skill args should parse");

    match cli.command {
        Commands::McpHub {
            cmd: McpHubCommands::GenSkill { socket },
        } => {
            assert_eq!(
                socket.as_deref(),
                Some("/tmp/cli-sub-agent-1000/mcp-hub.sock")
            );
        }
        _ => panic!("expected mcp-hub gen-skill subcommand"),
    }
}

#[test]
fn review_help_shows_options() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["review", "--help"])
        .output()
        .expect("failed to run csa review --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Review code changes using an AI tool"));
    assert!(stdout.contains("--tool"));
    assert!(stdout.contains("--session"));
    assert!(stdout.contains("--diff"));
    assert!(stdout.contains("--branch"));
    assert!(stdout.contains("--commit"));
    assert!(stdout.contains("--model"));
}

#[test]
fn review_cli_validation_applies_red_team_defaults() {
    let cli = Cli::try_parse_from(["csa", "review", "--red-team", "--diff"])
        .expect("review args should parse");

    match &cli.command {
        Commands::Review(args) => {
            validate_command_args(&cli.command, 1800).expect("review args should validate");
            assert_eq!(args.effective_review_mode().as_str(), "red-team");
            assert_eq!(args.effective_security_mode(), "on");
        }
        _ => panic!("expected review subcommand"),
    }
}

#[test]
fn review_cli_validation_rejects_red_team_with_security_off() {
    let cli = Cli::try_parse_from([
        "csa",
        "review",
        "--red-team",
        "--diff",
        "--security-mode",
        "off",
    ])
    .expect("review args should parse before validation");

    let err =
        validate_command_args(&cli.command, 1800).expect_err("validation should reject conflict");
    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}

// ---------------------------------------------------------------------------
// Smoke tests for subcommands that do NOT launch real LLM tools.
// ---------------------------------------------------------------------------

#[test]
fn config_show_exits_zero_after_init_minimal() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_project(tmp.path());

    let output = csa_cmd(tmp.path())
        .args(["config", "show"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config show");

    assert!(output.status.success(), "csa config show should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("schema_version"),
        "should contain schema_version"
    );
    assert!(
        stdout.contains("[project]"),
        "should contain [project] section"
    );
}

#[test]
fn config_show_exits_zero_after_init_full() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_project_full(tmp.path());

    let output = csa_cmd(tmp.path())
        .args(["config", "show"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config show");

    assert!(output.status.success(), "csa config show should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("schema_version"),
        "should contain schema_version"
    );
    assert!(
        stdout.contains("[project]"),
        "should contain [project] section"
    );
    assert!(
        stdout.contains("[tools"),
        "should contain [tools.*] sections"
    );
}

#[test]
fn config_get_resolves_nested_resource_keys_from_effective_display_tree() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[resources]
memory_max_mb = 1024
"#,
    )
    .expect("write config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "resources.slot_wait_timeout_seconds"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get resources.slot_wait_timeout_seconds");

    assert!(output.status.success(), "config get should exit 0");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "250");
}

#[test]
fn config_get_project_only_resolves_effective_project_defaults() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[resources]
memory_max_mb = 1024
"#,
    )
    .expect("write config");

    let output = csa_cmd(tmp.path())
        .args([
            "config",
            "get",
            "resources.slot_wait_timeout_seconds",
            "--project",
        ])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get resources.slot_wait_timeout_seconds --project");

    assert!(
        output.status.success(),
        "project-only config get should exit 0"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "250");
}

#[test]
fn config_get_prefers_effective_tool_state_over_raw_project_value() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let global_config_path = global_config_path(tmp.path());
    let global_dir = global_config_path.parent().expect("global config dir");
    std::fs::create_dir_all(global_dir).expect("create global config dir");
    std::fs::write(
        &global_config_path,
        r#"
[tools.codex]
enabled = false
"#,
    )
    .expect("write global config");

    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[tools.codex]
enabled = true
"#,
    )
    .expect("write project config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "tools.codex.enabled"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get tools.codex.enabled");

    assert!(output.status.success(), "config get should exit 0");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "false");
}

#[test]
fn config_get_redacts_global_memory_api_keys_in_project_scoped_lookups() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let global_config_path = global_config_path(tmp.path());
    let global_dir = global_config_path.parent().expect("global config dir");
    std::fs::create_dir_all(global_dir).expect("create global config dir");
    std::fs::write(
        &global_config_path,
        r#"
[memory.llm]
enabled = true
api_key = "sk-super-secret-5982"
"#,
    )
    .expect("write global config");

    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[memory]
inject = true
"#,
    )
    .expect("write project config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "memory"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get memory");

    assert!(output.status.success(), "config get should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("sk-super-secret-5982"),
        "config get leaked raw api key: {stdout}"
    );
    assert!(
        stdout.contains("api_key") && stdout.contains("..."),
        "config get should render a masked api key: {stdout}"
    );
}

#[test]
fn config_get_falls_back_to_raw_project_value_when_global_config_is_invalid() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let global_config_path = global_config_path(tmp.path());
    let global_dir = global_config_path.parent().expect("global config dir");
    std::fs::create_dir_all(global_dir).expect("create global config dir");
    std::fs::write(&global_config_path, "{{invalid toml").expect("write invalid global config");

    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[resources]
memory_max_mb = 1024
"#,
    )
    .expect("write project config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "resources.memory_max_mb"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get resources.memory_max_mb");

    assert!(output.status.success(), "config get should exit 0");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1024");
}

#[test]
fn config_get_reads_unknown_raw_project_sections() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[pr_review]
cloud_bot_name = "gemini-code-assist"
cloud_bot_trigger = "comment"
merge_strategy = "merge"
delete_branch = false
"#,
    )
    .expect("write config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "pr_review.cloud_bot_name"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get pr_review.cloud_bot_name");

    assert!(output.status.success(), "config get should exit 0");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "gemini-code-assist"
    );
}

#[test]
fn config_get_returns_default_for_missing_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "missing.key", "--default", "fallback"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get --default");

    assert!(
        output.status.success(),
        "config get --default should exit 0"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "fallback");
}

#[test]
fn config_get_suggests_close_matches_for_missing_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[resources]
memory_max_mb = 1024
"#,
    )
    .expect("write config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "resources.slot_wait_timeout_second"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get with typo");

    assert!(!output.status.success(), "config get typo should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Closest matches:"),
        "stderr should include suggestions, got: {stderr}"
    );
    assert!(
        stderr.contains("resources.slot_wait_timeout_seconds"),
        "stderr should mention the closest key, got: {stderr}"
    );
}

#[test]
fn gc_dry_run_exits_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["gc", "--dry-run"])
        .output()
        .expect("failed to run csa gc --dry-run");

    assert!(output.status.success(), "csa gc --dry-run should exit 0");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        combined.contains("dry-run"),
        "output should mention dry-run mode"
    );
}

#[test]
fn tiers_list_exits_zero_after_init_full() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_project_full(tmp.path());

    let output = csa_cmd(tmp.path())
        .args(["tiers", "list"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa tiers list");

    assert!(output.status.success(), "csa tiers list should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Full init defines at least these tiers.
    assert!(stdout.contains("tier-1"), "should list tier-1");
    assert!(stdout.contains("tier-2"), "should list tier-2");
    assert!(stdout.contains("tier-3"), "should list tier-3");
}

#[test]
fn skill_list_exits_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["skill", "list"])
        .output()
        .expect("failed to run csa skill list");

    assert!(output.status.success(), "csa skill list should exit 0");
}

#[test]
fn session_list_exits_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["session", "list"])
        .output()
        .expect("failed to run csa session list");

    assert!(output.status.success(), "csa session list should exit 0");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        combined.contains("No sessions found"),
        "empty state should report no sessions"
    );
}

#[test]
fn run_direct_tool_tier_rejection_surfaces_cause_and_session_id() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_project_config_with_tier(tmp.path());

    let output = csa_cmd(tmp.path())
        .args([
            "run",
            "--sa-mode",
            "true",
            "--no-daemon",
            "--tool",
            "codex",
            "--no-idle-timeout",
            "--timeout",
            "1800",
            "inspect the repository",
        ])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa run");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1, got {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Direct --tool is blocked when tiers are configured"),
        "user-facing cause should be shown, stderr: {stderr}"
    );
    assert!(
        stderr.contains("--auto-route <intent>"),
        "actionable guidance should remain visible, stderr: {stderr}"
    );
    assert!(
        stderr.contains("Session ID:"),
        "session id should be preserved for diagnostics, stderr: {stderr}"
    );
    assert!(
        !stderr
            .lines()
            .find(|line| line.starts_with("Error:"))
            .unwrap_or_default()
            .contains("meta_session_id="),
        "top-level error line should not be opaque metadata, stderr: {stderr}"
    );
}

#[test]
fn test_audit_help() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["audit", "--help"])
        .output()
        .expect("failed to run csa audit --help");

    assert!(output.status.success(), "csa audit --help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Manage audit manifest lifecycle"));
    assert!(stdout.contains("init"));
    assert!(stdout.contains("status"));
    assert!(stdout.contains("sync"));
}

#[test]
fn test_audit_init_parse() {
    let cli = Cli::try_parse_from(["csa", "audit", "init", "--root", "."])
        .expect("audit init args should parse");

    match cli.command {
        Commands::Audit {
            command:
                AuditCommands::Init {
                    root,
                    ignore,
                    mirror_dir,
                },
        } => {
            assert_eq!(root, ".");
            assert!(ignore.is_empty());
            assert!(mirror_dir.is_none());
        }
        _ => panic!("expected audit init subcommand"),
    }
}

#[test]
fn test_audit_status_parse() {
    let cli = Cli::try_parse_from(["csa", "audit", "status", "--format", "json"])
        .expect("audit status args should parse");

    match cli.command {
        Commands::Audit {
            command:
                AuditCommands::Status {
                    format,
                    filter,
                    order,
                },
        } => {
            assert!(matches!(format, OutputFormat::Json));
            assert_eq!(filter, None);
            assert_eq!(order, "topo");
        }
        _ => panic!("expected audit status subcommand"),
    }
}
