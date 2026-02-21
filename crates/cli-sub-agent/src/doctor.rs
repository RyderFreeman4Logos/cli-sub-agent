//! Environment diagnostics for CSA.

use anyhow::Result;
use csa_config::{ProjectConfig, paths};
use csa_core::types::OutputFormat;
use csa_resource::rlimit::{current_rlimit_as, current_rlimit_nproc};
use csa_resource::sandbox::{SandboxCapability, detect_sandbox_capability, systemd_version};
use std::env;
use std::process::Command;
use sysinfo::System;

/// Tool availability status.
#[derive(Debug)]
struct ToolStatus {
    name: &'static str,
    installed: bool,
    version: Option<String>,
}

/// Get installation hint for a tool.
fn install_hint(tool_name: &str) -> &'static str {
    match tool_name {
        "gemini-cli" => "npm install -g @google/gemini-cli",
        "opencode" => "go install github.com/sst/opencode@latest",
        "codex" => "npm install -g @zed-industries/codex-acp",
        "claude-code" => "npm install -g @zed-industries/claude-code-acp",
        _ => "unknown tool",
    }
}

/// Run full environment diagnostics.
pub async fn run_doctor(format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => run_doctor_json().await,
        OutputFormat::Text => run_doctor_text().await,
    }
}

/// Run diagnostics with human-readable text output.
async fn run_doctor_text() -> Result<()> {
    println!("=== CSA Environment Check ===");
    print_platform_info();
    print_state_dir();
    println!();

    println!("=== Tool Availability ===");
    print_tool_availability().await;
    println!();

    println!("=== Project Config ===");
    print_project_config()?;
    println!();

    println!("=== Resource Status ===");
    print_resource_status();
    println!();

    println!("=== Sandbox ===");
    print_sandbox_status();

    Ok(())
}

/// Run diagnostics with JSON output.
async fn run_doctor_json() -> Result<()> {
    let os = env::consts::OS;
    let arch = env::consts::ARCH;
    let version = env!("CARGO_PKG_VERSION");

    let state_dir = paths::state_dir()
        .map(|d| d.display().to_string())
        .unwrap_or_default();

    // Check tools
    let tools = [
        ("gemini-cli", "gemini"),
        ("opencode", "opencode"),
        ("codex", "codex-acp"),
        ("claude-code", "claude-code-acp"),
    ];

    let tool_statuses: Vec<serde_json::Value> = tools
        .iter()
        .map(|(tool_name, exe_name)| {
            let status = check_tool_status(tool_name, exe_name);
            serde_json::json!({
                "name": status.name,
                "installed": status.installed,
                "version": status.version,
            })
        })
        .collect();

    // Check project config
    let cwd = env::current_dir()?;
    let config_status = match ProjectConfig::load(&cwd) {
        Ok(Some(config)) => {
            let mut enabled = Vec::new();
            let mut disabled = Vec::new();
            for tool_name in &["gemini-cli", "opencode", "codex", "claude-code"] {
                if config.is_tool_enabled(tool_name) {
                    enabled.push(*tool_name);
                } else {
                    disabled.push(*tool_name);
                }
            }
            serde_json::json!({
                "found": true,
                "valid": true,
                "enabled_tools": enabled,
                "disabled_tools": disabled,
            })
        }
        Ok(None) => serde_json::json!({
            "found": false,
            "valid": false,
        }),
        Err(e) => serde_json::json!({
            "found": true,
            "valid": false,
            "error": e.to_string(),
        }),
    };

    // Resource status
    let mut sys = System::new();
    sys.refresh_memory();
    let available_memory = sys.available_memory();
    let free_swap = sys.free_swap();

    // Sandbox detection
    let cap = detect_sandbox_capability();
    let sandbox_status = match cap {
        SandboxCapability::CgroupV2 => serde_json::json!({
            "capability": "CgroupV2",
            "systemd_version": systemd_version(),
            "user_scope": true,
        }),
        SandboxCapability::Setrlimit => serde_json::json!({
            "capability": "Setrlimit",
            "rlimit_as_mb": current_rlimit_as(),
            "rlimit_nproc": current_rlimit_nproc(),
        }),
        SandboxCapability::None => serde_json::json!({
            "capability": "None",
        }),
    };

    let result = serde_json::json!({
        "platform": {
            "os": os,
            "arch": arch,
        },
        "csa_version": version,
        "state_dir": state_dir,
        "tools": tool_statuses,
        "config": config_status,
        "resources": {
            "available_memory_bytes": available_memory,
            "free_swap_bytes": free_swap,
            "total_free_bytes": available_memory.saturating_add(free_swap),
        },
        "sandbox": sandbox_status,
    });

    println!("{}", serde_json::to_string_pretty(&result)?);

    Ok(())
}

/// Print platform information.
fn print_platform_info() {
    let os = env::consts::OS;
    let arch = env::consts::ARCH;
    let version = env!("CARGO_PKG_VERSION");

    println!("Platform:    {} {}", os, arch);
    println!("CSA Version: {}", version);
}

/// Print CSA state directory path.
fn print_state_dir() {
    if let Some(state_dir) = paths::state_dir() {
        println!("State Dir:   {}", state_dir.display());
    } else {
        println!("State Dir:   (unable to determine)");
    }
}

/// Check and print tool availability for all 4 tools.
async fn print_tool_availability() {
    let tools = [
        ("gemini-cli", "gemini"),
        ("opencode", "opencode"),
        ("codex", "codex-acp"),
        ("claude-code", "claude-code-acp"),
    ];

    let mut installed_count = 0;
    let total_count = tools.len();

    for (tool_name, exe_name) in &tools {
        let status = check_tool_status(tool_name, exe_name);
        if status.installed {
            installed_count += 1;
        }
        print_tool_status(&status);
    }

    // Print summary
    println!();
    println!("{}/{} tools ready", installed_count, total_count);
}

/// Check if a tool is installed and get its version.
fn check_tool_status(tool_name: &'static str, exe_name: &str) -> ToolStatus {
    // First check if executable exists in PATH
    let installed = Command::new("which")
        .arg(exe_name)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);

    if !installed {
        return ToolStatus {
            name: tool_name,
            installed: false,
            version: None,
        };
    }

    // Try to get version
    let version = check_tool_version(exe_name);

    ToolStatus {
        name: tool_name,
        installed: true,
        version,
    }
}

/// Try to get tool version by running `<exe> --version`.
fn check_tool_version(exe_name: &str) -> Option<String> {
    let output = Command::new(exe_name).arg("--version").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Take first line and trim
    stdout.lines().next().map(|s| s.trim().to_string())
}

/// Print a single tool's status.
fn print_tool_status(status: &ToolStatus) {
    let checkmark = if status.installed { "✓" } else { "✗" };
    let status_msg = if status.installed {
        if let Some(ref version) = status.version {
            format!("installed ({})", version)
        } else {
            "installed (version unknown)".to_string()
        }
    } else {
        "not found".to_string()
    };

    println!(
        "{:<12} {} {}",
        format!("{}:", status.name),
        checkmark,
        status_msg
    );

    // Print install hint if not found
    if !status.installed {
        let hint = install_hint(status.name);
        println!("             Install: {}", hint);
    }
}

/// Print project config status.
fn print_project_config() -> Result<()> {
    let cwd = env::current_dir()?;
    let config_path = cwd.join(".csa").join("config.toml");

    if !config_path.exists() {
        println!("Config:      .csa/config.toml (missing)");
        println!("             Run 'csa init' to create configuration");
        return Ok(());
    }

    // Try to load config
    match ProjectConfig::load(&cwd) {
        Ok(Some(config)) => {
            println!("Config:      .csa/config.toml (valid)");

            // List enabled and disabled tools
            let mut enabled = Vec::new();
            let mut disabled = Vec::new();

            for tool_name in &["gemini-cli", "opencode", "codex", "claude-code"] {
                if config.is_tool_enabled(tool_name) {
                    enabled.push(*tool_name);
                } else {
                    disabled.push(*tool_name);
                }
            }

            if !enabled.is_empty() {
                println!("Enabled:     {}", enabled.join(", "));
            }
            if !disabled.is_empty() {
                println!("Disabled:    {}", disabled.join(", "));
            }
        }
        Ok(None) => {
            println!("Config:      .csa/config.toml (missing)");
        }
        Err(e) => {
            println!("Config:      .csa/config.toml (invalid)");
            println!("             Error: {}", e);
        }
    }

    Ok(())
}

/// Print resource status (combined available physical + free swap memory).
fn print_resource_status() {
    let mut sys = System::new();
    sys.refresh_memory();

    let available_memory_bytes = sys.available_memory();
    let free_swap_bytes = sys.free_swap();
    let total_free = available_memory_bytes.saturating_add(free_swap_bytes);

    println!(
        "Available Memory: {} (physical {} + swap {})",
        format_bytes(total_free),
        format_bytes(available_memory_bytes),
        format_bytes(free_swap_bytes),
    );
}

/// Print sandbox capability status.
fn print_sandbox_status() {
    let cap = detect_sandbox_capability();
    println!("Capability:  {}", cap);

    match cap {
        SandboxCapability::CgroupV2 => {
            if let Some(ver) = systemd_version() {
                println!("Systemd:     {}", ver);
            }
            println!("User scope:  supported");
        }
        SandboxCapability::Setrlimit => {
            match current_rlimit_as() {
                Some(mb) => println!("RLIMIT_AS:   {} MB", mb),
                None => println!("RLIMIT_AS:   unlimited"),
            }
            match current_rlimit_nproc() {
                Some(n) => println!("RLIMIT_NPROC: {}", n),
                None => println!("RLIMIT_NPROC: unlimited"),
            }
        }
        SandboxCapability::None => {
            println!("Warning:     No sandbox isolation available.");
            println!("             Resource limits will not be enforced.");
        }
    }
}

/// Format bytes as human-readable string (GB).
fn format_bytes(bytes: u64) -> String {
    let gb = bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    format!("{:.1} GB", gb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0.0 GB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(format_bytes(8 * 1024 * 1024 * 1024), "8.0 GB");
        assert_eq!(format_bytes(8589934592), "8.0 GB"); // 8 GB in bytes
    }

    #[test]
    fn test_check_tool_version_nonexistent() {
        let version = check_tool_version("nonexistent-tool-12345");
        assert!(version.is_none());
    }

    #[test]
    fn test_check_tool_status_nonexistent() {
        let status = check_tool_status("nonexistent-tool", "nonexistent-exe-12345");
        assert!(!status.installed);
        assert!(status.version.is_none());
    }
}
