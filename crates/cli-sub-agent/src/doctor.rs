//! Environment diagnostics for CSA.

use anyhow::Result;
use csa_config::ProjectConfig;
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
        "codex" => "npm install -g @openai/codex",
        "claude-code" => "npm install -g @anthropic-ai/claude-code",
        _ => "unknown tool",
    }
}

/// Run full environment diagnostics.
pub async fn run_doctor() -> Result<()> {
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
    if let Some(proj_dirs) = directories::ProjectDirs::from("", "", "csa") {
        if let Some(state_dir) = proj_dirs.state_dir() {
            println!("State Dir:   {}", state_dir.display());
        } else {
            println!("State Dir:   (unable to determine)");
        }
    } else {
        println!("State Dir:   (unable to determine)");
    }
}

/// Check and print tool availability for all 4 tools.
async fn print_tool_availability() {
    let tools = [
        ("gemini-cli", "gemini"),
        ("opencode", "opencode"),
        ("codex", "codex"),
        ("claude-code", "claude"),
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

/// Print resource status (free memory and swap).
fn print_resource_status() {
    let mut sys = System::new_all();
    sys.refresh_memory();

    let free_memory_bytes = sys.available_memory();
    let free_swap_bytes = sys.free_swap();

    println!("Free Memory: {}", format_bytes(free_memory_bytes));
    println!("Free Swap:   {}", format_bytes(free_swap_bytes));
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
