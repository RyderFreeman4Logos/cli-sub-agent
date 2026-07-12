//! Environment diagnostics for CSA.

use anyhow::Result;
use csa_config::{ProjectConfig, paths};
use csa_core::types::{OutputFormat, PRIMARY_TOOL_NAMES};
use csa_resource::filesystem_sandbox::detect_filesystem_capability;
use csa_resource::rlimit::current_rlimit_nproc;
use csa_resource::sandbox::{ResourceCapability, detect_resource_capability, systemd_version};
use std::env;
use std::path::{Path, PathBuf};
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
use crate::install_provenance;
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
    config_enabled: bool,
    availability: ToolAvailabilityState,
    binary_name: String,
    version: Option<String>,
    hint: Option<String>,
    transport: Option<ToolTransportDoctorStatus>,
}

impl ToolStatus {
    fn binary_available(&self) -> bool {
        matches!(self.availability, ToolAvailabilityState::Installed)
    }

    fn is_ready(&self) -> bool {
        self.config_enabled && self.binary_available()
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

/// Dispatch `csa doctor` / `csa doctor <subcommand>`.
pub async fn dispatch_doctor(
    format: OutputFormat,
    subcommand: Option<crate::cli::DoctorSubcommand>,
) -> Result<()> {
    match subcommand {
        None => run_doctor(format).await,
        Some(crate::cli::DoctorSubcommand::Install { target, artifact }) => {
            run_doctor_install(&target, artifact.as_deref())
        }
        Some(crate::cli::DoctorSubcommand::Routing { operation, tier }) => {
            run_doctor_routing(format, operation, tier).await
        }
    }
}

/// Report the same install provenance used by `just install`.
pub(crate) fn run_doctor_install(target: &Path, artifact: Option<&Path>) -> Result<()> {
    let artifact = artifact.unwrap_or(target);
    let report = install_provenance::inspect_current_path(artifact, target)?;
    println!("{}", report.diagnostic());
    if report.is_current() {
        Ok(())
    } else {
        anyhow::bail!("CSA installation provenance is not current")
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
    println!();

    println!("=== Install Provenance ===");
    print_install_provenance_status();

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
        Some(config) => PRIMARY_TOOL_NAMES
            .iter()
            .copied()
            .map(|tool_name| {
                let status = check_tool_status(tool_name, Some(config));
                tool_status_json(&status)
            })
            .collect(),
        None => {
            if effective_config_status.tool_availability_error().is_some() {
                Vec::new()
            } else {
                PRIMARY_TOOL_NAMES
                    .iter()
                    .copied()
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

    let install_status = install_provenance_json();

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
        "install": install_status,
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

/// Print the `=== Project Config ===` section.
///
/// This renders the RAW `.csa/config.toml` project config only; the effective
/// (merged) runtime enablement gate is reported by the per-tool
/// `=== Tool Availability ===` blocks instead (#1752 / #1836).
fn print_project_config(status: &DoctorProjectConfigStatus) {
    for line in render_project_config_lines(status) {
        println!("{line}");
    }
}

/// Side-effect-free PATH/install provenance summary for `csa doctor`.
///
/// Does not mutate PATH entries. Failures (missing `csa`, hash errors) are
/// reported as text rather than failing the whole doctor command.
fn print_install_provenance_status() {
    println!("{}", install_provenance_snapshot().render_text());
}

fn install_provenance_json() -> serde_json::Value {
    install_provenance_snapshot().to_json()
}

/// Snapshot used by both text and JSON doctor surfaces.
///
/// When the intended install target exists it is compared against the
/// PATH-resolved executable (same logic as `just install` without `--artifact`).
/// When it is missing, the PATH-resolved binary is still reported and marked
/// non-current so shadowing stays visible.
fn install_provenance_snapshot() -> InstallDoctorSnapshot {
    let intended = PathBuf::from("/usr/local/bin/csa");
    if intended.is_file() {
        match install_provenance::inspect_current_path(&intended, &intended) {
            Ok(report) => InstallDoctorSnapshot::Report(report),
            Err(error) => InstallDoctorSnapshot::Unavailable {
                intended_target: intended,
                error: error.to_string(),
            },
        }
    } else {
        match install_provenance::resolve_current_path() {
            Ok(path_resolved) => {
                let version = install_provenance::version_output_for(&path_resolved)
                    .unwrap_or_else(|error| format!("(unavailable: {error})"));
                InstallDoctorSnapshot::MissingIntended {
                    path_resolved,
                    intended_target: intended,
                    version_output: version,
                }
            }
            Err(error) => InstallDoctorSnapshot::Unavailable {
                intended_target: intended,
                error: error.to_string(),
            },
        }
    }
}

enum InstallDoctorSnapshot {
    Report(install_provenance::InstallProvenanceReport),
    MissingIntended {
        path_resolved: PathBuf,
        intended_target: PathBuf,
        version_output: String,
    },
    Unavailable {
        intended_target: PathBuf,
        error: String,
    },
}

impl InstallDoctorSnapshot {
    fn render_text(&self) -> String {
        match self {
            Self::Report(report) => report.diagnostic(),
            Self::MissingIntended {
                path_resolved,
                intended_target,
                version_output,
            } => format!(
                "CSA install provenance: intended install target is missing\n  PATH-resolved executable: {}\n  intended install target: {}\n  PATH-resolved version/source commit: {}\n  status: not current\n  remediation: run `just install` (or install to the intended target); CSA will not overwrite arbitrary PATH entries.",
                path_resolved.display(),
                intended_target.display(),
                version_output
            ),
            Self::Unavailable {
                intended_target,
                error,
            } => format!(
                "Install provenance unavailable: {error}\n  intended install target: {}\n  remediation: ensure `csa` is on PATH or run `just install`.",
                intended_target.display()
            ),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Report(report) => serde_json::json!({
                "status": match report.status {
                    install_provenance::InstallProvenanceStatus::Current => "current",
                    install_provenance::InstallProvenanceStatus::StaleShadow => "stale_shadow",
                    install_provenance::InstallProvenanceStatus::UnsafeShadow => "unsafe_shadow",
                },
                "path_resolved": report.path_resolved.display().to_string(),
                "intended_target": report.intended_target.display().to_string(),
                "artifact": report.artifact.display().to_string(),
                "artifact_sha256": report.artifact_hash,
                "path_resolved_sha256": report.resolved_hash,
                "artifact_version": report.artifact_version,
                "path_resolved_version": report.version_output,
                "current": report.is_current(),
            }),
            Self::MissingIntended {
                path_resolved,
                intended_target,
                version_output,
            } => serde_json::json!({
                "status": "missing_intended_target",
                "path_resolved": path_resolved.display().to_string(),
                "intended_target": intended_target.display().to_string(),
                "path_resolved_version": version_output,
                "current": false,
            }),
            Self::Unavailable {
                intended_target,
                error,
            } => serde_json::json!({
                "status": "unavailable",
                "intended_target": intended_target.display().to_string(),
                "error": error,
                "current": false,
            }),
        }
    }
}

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;
