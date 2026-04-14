//! Environment diagnostics for CSA.

use anyhow::Result;
use csa_config::{ProjectConfig, ToolTransport, paths};
use csa_core::types::OutputFormat;
use csa_executor::{CodexRuntimeMetadata, CodexTransport};
use csa_resource::filesystem_sandbox::{FilesystemCapability, detect_filesystem_capability};
use csa_resource::rlimit::current_rlimit_nproc;
use csa_resource::sandbox::{ResourceCapability, detect_resource_capability, systemd_version};
use std::env;
use std::path::Path;
use std::process::Command;
use sysinfo::System;

#[path = "doctor_routing.rs"]
mod doctor_routing;
pub use doctor_routing::run_doctor_routing;

/// Tool availability status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolAvailabilityState {
    Installed,
    Missing,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexDoctorStatus {
    transport_active: CodexTransport,
    acp_compiled_in: bool,
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
    codex_transport: Option<CodexDoctorStatus>,
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

impl DoctorProjectConfigStatus {
    fn project_config(&self) -> Option<&ProjectConfig> {
        match self {
            Self::Valid(config) => Some(config),
            Self::Missing | Self::Invalid(_) => None,
        }
    }

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

fn project_config_tool_lists(config: &ProjectConfig) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut enabled = Vec::new();
    let mut disabled = Vec::new();

    for tool_name in &["gemini-cli", "opencode", "codex", "claude-code"] {
        if config.is_tool_enabled(tool_name) {
            enabled.push(*tool_name);
        } else {
            disabled.push(*tool_name);
        }
    }

    (enabled, disabled)
}

fn render_project_config_lines(status: &DoctorProjectConfigStatus) -> Vec<String> {
    match status {
        DoctorProjectConfigStatus::Missing => vec![
            "Config:      .csa/config.toml (missing)".to_string(),
            "             Run 'csa init' to create configuration".to_string(),
        ],
        DoctorProjectConfigStatus::Valid(config) => {
            let (enabled, disabled) = project_config_tool_lists(config);
            let mut lines = vec!["Config:      .csa/config.toml (valid)".to_string()];

            if !enabled.is_empty() {
                lines.push(format!("Enabled:     {}", enabled.join(", ")));
            }
            if !disabled.is_empty() {
                lines.push(format!("Disabled:    {}", disabled.join(", ")));
            }

            lines
        }
        DoctorProjectConfigStatus::Invalid(error) => vec![
            "Config:      .csa/config.toml (invalid)".to_string(),
            format!("             Error: {error}"),
        ],
    }
}

fn tool_exe_name(tool_name: &str, config: Option<&ProjectConfig>) -> String {
    crate::run_helpers::resolved_tool_binary_name(tool_name, config)
        .unwrap_or(tool_name)
        .to_string()
}

fn resolved_codex_transport(config: Option<&ProjectConfig>) -> CodexTransport {
    config
        .and_then(|cfg| cfg.tool_transport("codex"))
        .map(|transport| match transport {
            ToolTransport::Cli => CodexTransport::Cli,
            ToolTransport::Acp => CodexTransport::Acp,
        })
        .unwrap_or_else(CodexTransport::default_for_build)
}

fn load_doctor_project_config_from(project_root: &Path) -> Result<Option<ProjectConfig>> {
    ProjectConfig::load(project_root)
}

fn inspect_doctor_project_config_from(project_root: &Path) -> DoctorProjectConfigStatus {
    let config_path = project_root.join(".csa").join("config.toml");
    if !config_path.exists() {
        return DoctorProjectConfigStatus::Missing;
    }

    match load_doctor_project_config_from(project_root) {
        Ok(Some(config)) => DoctorProjectConfigStatus::Valid(Box::new(config)),
        Ok(None) => DoctorProjectConfigStatus::Missing,
        Err(error) => DoctorProjectConfigStatus::Invalid(error.to_string()),
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

    println!("=== CSA Environment Check ===");
    print_platform_info();
    print_state_dir();
    println!();

    println!("=== Tool Availability ===");
    print_tool_availability(project_config_status.project_config()).await;
    println!();

    println!("=== Project Config ===");
    print_project_config(&project_config_status);
    println!();

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

    let state_dir = paths::state_dir()
        .map(|d| d.display().to_string())
        .unwrap_or_default();

    let tool_statuses: Vec<serde_json::Value> = ["gemini-cli", "opencode", "codex", "claude-code"]
        .iter()
        .map(|tool_name| {
            let status = check_tool_status(tool_name, project_config_status.project_config());
            tool_status_json(&status)
        })
        .collect();

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
        "config": project_config_status.json_value(),
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

/// Check and print tool availability for all 4 tools.
async fn print_tool_availability(config: Option<&ProjectConfig>) {
    let tools = ["gemini-cli", "opencode", "codex", "claude-code"];

    let mut installed_count = 0;
    let total_count = tools.len();

    for tool_name in &tools {
        let status = check_tool_status(tool_name, config);
        if status.is_ready() {
            installed_count += 1;
        }
        print_tool_status(&status);
    }

    // Print summary
    println!();
    println!("{installed_count}/{total_count} tools ready");
}

/// Check if a tool is installed and get its version.
fn check_tool_status(tool_name: &'static str, config: Option<&ProjectConfig>) -> ToolStatus {
    let binary_name = tool_exe_name(tool_name, config);
    match crate::run_helpers::tool_binary_availability(tool_name, config) {
        crate::run_helpers::ToolBinaryAvailability::Available { .. } => ToolStatus {
            name: tool_name,
            availability: ToolAvailabilityState::Installed,
            binary_name: binary_name.clone(),
            version: check_tool_version(&binary_name),
            hint: None,
            codex_transport: codex_doctor_status(tool_name, config),
        },
        crate::run_helpers::ToolBinaryAvailability::Missing { hint, .. } => ToolStatus {
            name: tool_name,
            availability: ToolAvailabilityState::Missing,
            binary_name,
            version: None,
            hint: Some(hint.into_owned()),
            codex_transport: codex_doctor_status(tool_name, config),
        },
        crate::run_helpers::ToolBinaryAvailability::Unsupported { hint, .. } => ToolStatus {
            name: tool_name,
            availability: ToolAvailabilityState::Unsupported,
            binary_name,
            version: None,
            hint: Some(hint.into_owned()),
            codex_transport: codex_doctor_status(tool_name, config),
        },
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
    for line in render_tool_status_lines(status) {
        println!("{line}");
    }
}

fn render_tool_status_lines(status: &ToolStatus) -> Vec<String> {
    let checkmark = if status.is_ready() { "✓" } else { "✗" };
    let status_msg = match status.availability {
        ToolAvailabilityState::Installed => status
            .version
            .as_ref()
            .map(|version| format!("installed ({version})"))
            .unwrap_or_else(|| "installed (version unknown)".to_string()),
        ToolAvailabilityState::Missing => "not found".to_string(),
        ToolAvailabilityState::Unsupported => "unsupported".to_string(),
    };

    let mut lines = vec![format!(
        "{:<12} {} {}",
        format!("{}:", status.name),
        checkmark,
        status_msg
    )];

    if let Some(codex_status) = status.codex_transport.as_ref() {
        lines.push(format!(
            "             Active transport: {}",
            codex_transport_label(codex_status.transport_active)
        ));
        lines.push(format!(
            "             ACP compiled in: {}",
            yes_no(codex_status.acp_compiled_in)
        ));
        lines.push(format!(
            "             Probed binary: {}",
            codex_status.probed_binary
        ));
        if let Some(acp_override_hint) = codex_status.acp_override_hint {
            lines.push(format!("             ACP override: {acp_override_hint}"));
        }
    }

    if !status.is_ready()
        && let Some(hint) = status.hint.as_deref()
    {
        lines.push(format!(
            "             Expected runtime: {}",
            status.binary_name
        ));
        lines.push(format!("             {hint}"));
    }

    lines
}

fn tool_status_json(status: &ToolStatus) -> serde_json::Value {
    let mut entry = serde_json::json!({
        "name": status.name,
        "binary": status.binary_name,
        "installed": status.is_ready(),
        "version": status.version,
        "hint": status.hint,
    });

    if let Some(codex_status) = status.codex_transport.as_ref()
        && let Some(object) = entry.as_object_mut()
    {
        object.insert(
            "transport_active".to_string(),
            serde_json::json!(codex_transport_label(codex_status.transport_active)),
        );
        object.insert(
            "acp_compiled_in".to_string(),
            serde_json::json!(codex_status.acp_compiled_in),
        );
        object.insert(
            "probed_binary".to_string(),
            serde_json::json!(codex_status.probed_binary),
        );
    }

    entry
}

fn codex_doctor_status(
    tool_name: &str,
    config: Option<&ProjectConfig>,
) -> Option<CodexDoctorStatus> {
    if tool_name != "codex" {
        return None;
    }

    let transport_active = resolved_codex_transport(config);
    let acp_compiled_in = CodexRuntimeMetadata::acp_compiled_in();

    Some(CodexDoctorStatus {
        transport_active,
        acp_compiled_in,
        probed_binary: transport_active.runtime_binary_name().to_string(),
        acp_override_hint: if acp_compiled_in && transport_active != CodexTransport::Acp {
            Some("set [tools.codex].transport = \"acp\"")
        } else {
            None
        },
    })
}

fn codex_transport_label(transport: CodexTransport) -> &'static str {
    match transport {
        CodexTransport::Cli => "cli",
        CodexTransport::Acp => "acp",
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// Print project config status.
fn print_project_config(status: &DoctorProjectConfigStatus) {
    for line in render_project_config_lines(status) {
        println!("{line}");
    }
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
    let cap = detect_resource_capability();
    println!("Capability:  {cap}");

    match cap {
        ResourceCapability::CgroupV2 => {
            if let Some(ver) = systemd_version() {
                println!("Systemd:     {ver}");
            }
            println!("User scope:  supported");
        }
        ResourceCapability::Setrlimit => {
            println!("Enforces:    PID limit only (RLIMIT_NPROC)");
            match current_rlimit_nproc() {
                Some(n) => println!("RLIMIT_NPROC: {n}"),
                None => println!("RLIMIT_NPROC: unlimited"),
            }
            println!("Memory:      via MemoryBalloon (not setrlimit)");
        }
        ResourceCapability::None => {
            println!("Warning:     No sandbox isolation available.");
            println!("             Resource limits will not be enforced.");
        }
    }
}

/// Print filesystem sandbox capability status.
fn print_filesystem_sandbox_status() {
    let fs_cap = detect_filesystem_capability();
    println!("Capability:  {fs_cap}");

    match fs_cap {
        FilesystemCapability::Bwrap => {
            if let Some(ver) = bwrap_version() {
                println!("bwrap:       {ver}");
            }
            println!("User NS:     available");
        }
        FilesystemCapability::Landlock => {
            let abi = csa_resource::landlock::detect_abi();
            println!("Landlock ABI: {abi:?}");
        }
        FilesystemCapability::None => {
            println!("Warning:     No filesystem isolation available.");
            // Print diagnostic details
            if let Some(ver) = bwrap_version() {
                println!("bwrap:       {ver} (installed but user namespaces blocked)");
            } else {
                println!("bwrap:       not installed");
            }
            if is_apparmor_userns_restricted() {
                println!("AppArmor:    restricts unprivileged user namespaces");
            }
            if !has_usable_user_namespaces() {
                println!("User NS:     unavailable");
            }
        }
    }
}

/// Build JSON object for filesystem sandbox status.
fn build_filesystem_sandbox_json(fs_cap: FilesystemCapability) -> serde_json::Value {
    match fs_cap {
        FilesystemCapability::Bwrap => {
            serde_json::json!({
                "capability": "Bwrap",
                "bwrap_version": bwrap_version(),
                "user_namespaces": true,
                "apparmor_userns_restricted": is_apparmor_userns_restricted(),
            })
        }
        FilesystemCapability::Landlock => {
            let abi = csa_resource::landlock::detect_abi();
            serde_json::json!({
                "capability": "Landlock",
                "landlock_abi": format!("{abi:?}"),
                "user_namespaces": has_usable_user_namespaces(),
                "apparmor_userns_restricted": is_apparmor_userns_restricted(),
            })
        }
        FilesystemCapability::None => {
            serde_json::json!({
                "capability": "None",
                "bwrap_installed": bwrap_version().is_some(),
                "user_namespaces": has_usable_user_namespaces(),
                "apparmor_userns_restricted": is_apparmor_userns_restricted(),
            })
        }
    }
}

/// Get bwrap version string, if installed.
fn bwrap_version() -> Option<String> {
    let output = Command::new("bwrap").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next().map(|s| s.trim().to_string())
}

/// Check whether AppArmor restricts unprivileged user namespaces.
fn is_apparmor_userns_restricted() -> bool {
    let path = std::path::Path::new("/proc/sys/kernel/apparmor_restrict_unprivileged_userns");
    std::fs::read_to_string(path)
        .map(|content| content.trim() == "1")
        .unwrap_or(false)
}

/// Check whether unprivileged user namespaces work.
fn has_usable_user_namespaces() -> bool {
    Command::new("unshare")
        .args(["-U", "true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Print merge guard installation status.
fn print_merge_guard_status() {
    match csa_hooks::detect_installed_guard() {
        Some(path) => {
            println!("merge guard: installed ({})", path.display());
        }
        None => {
            println!("merge guard: not installed");
            println!("  Hint: csa hooks install-merge-guard");
        }
    }
}

/// Print git hook installation status.
fn print_git_hook_status(project_root: &Path) {
    let hooks = [("pre-push", "Blocks push without csa review session")];
    for (hook_name, description) in hooks {
        let hook_path = project_root.join(".git/hooks").join(hook_name);
        if hook_path.is_file() {
            println!("{hook_name}:  installed ({description})");
        } else {
            println!("{hook_name}:  NOT INSTALLED — {description}");
            println!("  Hint: ln -sf ../../scripts/hooks/{hook_name} .git/hooks/{hook_name}");
        }
    }
}

/// Format bytes as human-readable string (GB).
fn format_bytes(bytes: u64) -> String {
    let gb = bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    format!("{gb:.1} GB")
}

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;
