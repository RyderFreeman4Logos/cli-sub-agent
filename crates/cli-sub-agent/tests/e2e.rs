// End-to-end tests for the csa binary.
// Requires actual tool installations for full testing.

#[path = "../src/cli.rs"]
mod cli_defs;

use clap::Parser;
use cli_defs::{AuditCommands, Cli, Commands, McpHubCommands};
use csa_core::types::OutputFormat;
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
