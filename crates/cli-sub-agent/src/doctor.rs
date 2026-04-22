//! Environment diagnostics for CSA.

use anyhow::Result;
use csa_config::{ProjectConfig, paths};
use csa_core::types::OutputFormat;
use csa_resource::filesystem_sandbox::detect_filesystem_capability;
use csa_resource::rlimit::current_rlimit_nproc;
use csa_resource::sandbox::{ResourceCapability, detect_resource_capability, systemd_version};
use std::env;
use std::path::Path;
use sysinfo::System;

#[path = "doctor_config.rs"]
mod doctor_config;
#[path = "doctor_output_helpers.rs"]
mod doctor_output_helpers;
#[path = "doctor_resource.rs"]
mod doctor_resource;
#[path = "doctor_routing.rs"]
mod doctor_routing;
#[path = "doctor_sandbox.rs"]
mod doctor_sandbox;
#[path = "doctor_tools.rs"]
mod doctor_tools;
use doctor_config::{
    inspect_doctor_effective_config_from, inspect_doctor_project_config_from,
    project_config_tool_lists, render_effective_config_lines, render_project_config_lines,
    render_tool_availability_error_lines,
};
use doctor_output_helpers::{print_effective_config, print_tool_availability_error};
use doctor_resource::print_resource_status;
pub use doctor_routing::run_doctor_routing;
use doctor_sandbox::{
    build_filesystem_sandbox_json, print_filesystem_sandbox_status, print_git_hook_status,
    print_merge_guard_status, print_sandbox_status,
};
use doctor_tools::{check_tool_status, print_tool_availability, tool_status_json};

#[cfg(test)]
use doctor_config::load_doctor_project_config_from;
#[cfg(test)]
use doctor_resource::format_bytes;
#[cfg(test)]
use doctor_tools::{check_tool_version, render_tool_status_lines};

/// Tool availability status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolAvailabilityState {
    Installed,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolTransportDoctorStatus {
    transport_active: &'static str,
    acp_compiled_in: Option<bool>,
    probed_binary: String,
    acp_override_hint: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolStatus {
    name: &'static str,
    availability: ToolAvailabilityState,
    binary_name: String,
    version: Option<String>,
    hint: Option<String>,
    transport: Option<ToolTransportDoctorStatus>,
}

impl ToolStatus {
    fn is_ready(&self) -> bool {
        matches!(self.availability, ToolAvailabilityState::Installed)
    }
}

#[derive(Debug)]
enum DoctorProjectConfigStatus {
    Missing,
    Valid(Box<ProjectConfig>),
    Invalid(String),
}

#[derive(Debug)]
enum DoctorEffectiveConfigStatus {
    Defaults,
    Valid(Box<ProjectConfig>),
    Invalid(String),
}

impl DoctorProjectConfigStatus {
    fn json_value(&self) -> serde_json::Value {
        match self {
            Self::Missing => serde_json::json!({
                "found": false,
                "valid": false,
            }),
            Self::Valid(config) => {
                let (enabled, disabled) = project_config_tool_lists(config);
                serde_json::json!({
                    "found": true,
                    "valid": true,
                    "enabled_tools": enabled,
                    "disabled_tools": disabled,
                })
            }
            Self::Invalid(error) => serde_json::json!({
                "found": true,
                "valid": false,
                "error": error,
            }),
        }
    }
}

impl DoctorEffectiveConfigStatus {
    fn runtime_config(&self) -> Option<&ProjectConfig> {
        match self {
            Self::Valid(config) => Some(config.as_ref()),
            Self::Defaults | Self::Invalid(_) => None,
        }
    }

    fn tool_availability_error(&self) -> Option<String> {
        match self {
            Self::Defaults | Self::Valid(_) => None,
            Self::Invalid(error) => Some(format!(
                "Tool availability unknown (effective config invalid): {error}"
            )),
        }
    }

    fn json_value(&self) -> serde_json::Value {
        match self {
            Self::Defaults => serde_json::json!({
                "loaded": false,
                "valid": true,
            }),
            Self::Valid(_) => serde_json::json!({
                "loaded": true,
                "valid": true,
            }),
            Self::Invalid(error) => serde_json::json!({
                "loaded": false,
                "valid": false,
                "error": error,
            }),
        }
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
    let cwd = env::current_dir()?;
    run_doctor_text_from(&cwd).await
}

async fn run_doctor_text_from(project_root: &Path) -> Result<()> {
    let project_config_status = inspect_doctor_project_config_from(project_root);
    let effective_config_status = inspect_doctor_effective_config_from(project_root);

    println!("=== CSA Environment Check ===");
    print_platform_info();
    print_state_dir();
    println!();

    println!("=== Tool Availability ===");
    match effective_config_status.runtime_config() {
        Some(config) => print_tool_availability(Some(config)).await,
        None => match effective_config_status.tool_availability_error() {
            Some(error) => print_tool_availability_error(&error),
            None => print_tool_availability(None).await,
        },
    }
    println!();

    println!("=== Project Config ===");
    print_project_config(&project_config_status);
    println!();

    if matches!(
        effective_config_status,
        DoctorEffectiveConfigStatus::Invalid(_)
    ) {
        println!("=== Effective Config ===");
        print_effective_config(&effective_config_status);
        println!();
    }

    println!("=== Resource Status ===");
    print_resource_status();
    println!();

    println!("=== Sandbox (Resource) ===");
    print_sandbox_status();
    println!();

    println!("=== Sandbox (Filesystem) ===");
    print_filesystem_sandbox_status();
    println!();

    println!("=== Git Hooks ===");
    print_git_hook_status(project_root);
    println!();

    println!("=== Merge Guard ===");
    print_merge_guard_status();

    Ok(())
}

/// Run diagnostics with JSON output.
async fn run_doctor_json() -> Result<()> {
    let cwd = env::current_dir()?;
    let result = build_doctor_json(&cwd);

    println!("{}", serde_json::to_string_pretty(&result)?);

    Ok(())
}

fn build_doctor_json(project_root: &Path) -> serde_json::Value {
    let os = env::consts::OS;
    let arch = env::consts::ARCH;
    let version = env!("CARGO_PKG_VERSION");
    let project_config_status = inspect_doctor_project_config_from(project_root);
    let effective_config_status = inspect_doctor_effective_config_from(project_root);

    let state_dir = paths::state_dir()
        .map(|d| d.display().to_string())
        .unwrap_or_default();

    let tool_statuses: Vec<serde_json::Value> = match effective_config_status.runtime_config() {
        Some(config) => ["gemini-cli", "opencode", "codex", "claude-code"]
            .iter()
            .map(|tool_name| {
                let status = check_tool_status(tool_name, Some(config));
                tool_status_json(&status)
            })
            .collect(),
        None => {
            if effective_config_status.tool_availability_error().is_some() {
                Vec::new()
            } else {
                ["gemini-cli", "opencode", "codex", "claude-code"]
                    .iter()
                    .map(|tool_name| {
                        let status = check_tool_status(tool_name, None);
                        tool_status_json(&status)
                    })
                    .collect()
            }
        }
    };

    // Resource status
    let mut sys = System::new();
    sys.refresh_memory();
    let available_memory = sys.available_memory();
    let free_swap = sys.free_swap();

    // Sandbox detection
    let cap = detect_resource_capability();
    let sandbox_status = match cap {
        ResourceCapability::CgroupV2 => serde_json::json!({
            "capability": "CgroupV2",
            "systemd_version": systemd_version(),
            "user_scope": true,
        }),
        ResourceCapability::Setrlimit => serde_json::json!({
            "capability": "Setrlimit",
            "enforces": "pids_only",
            "rlimit_nproc": current_rlimit_nproc(),
        }),
        ResourceCapability::None => serde_json::json!({
            "capability": "None",
        }),
    };

    // Filesystem sandbox detection
    let fs_cap = detect_filesystem_capability();
    let fs_sandbox_status = build_filesystem_sandbox_json(fs_cap);

    // Merge guard status
    let merge_guard_status = match csa_hooks::detect_installed_guard() {
        Some(path) => serde_json::json!({
            "installed": true,
            "path": path.display().to_string(),
        }),
        None => serde_json::json!({
            "installed": false,
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
        "tools_error": effective_config_status.tool_availability_error(),
        "config": project_config_status.json_value(),
        "effective_config": effective_config_status.json_value(),
        "resources": {
            "available_memory_bytes": available_memory,
            "free_swap_bytes": free_swap,
            "total_free_bytes": available_memory.saturating_add(free_swap),
        },
        "sandbox": sandbox_status,
        "filesystem_sandbox": fs_sandbox_status,
        "merge_guard": merge_guard_status,
    });

    result
}

/// Print platform information.
fn print_platform_info() {
    let os = env::consts::OS;
    let arch = env::consts::ARCH;
    let version = env!("CARGO_PKG_VERSION");

    println!("Platform:    {os} {arch}");
    println!("CSA Version: {version}");
}

/// Print CSA state directory path.
fn print_state_dir() {
    if let Some(state_dir) = paths::state_dir() {
        println!("State Dir:   {}", state_dir.display());
    } else {
        println!("State Dir:   (unable to determine)");
    }
}

/// Print project config status.
fn print_project_config(status: &DoctorProjectConfigStatus) {
    for line in render_project_config_lines(status) {
        println!("{line}");
    }
}

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;
