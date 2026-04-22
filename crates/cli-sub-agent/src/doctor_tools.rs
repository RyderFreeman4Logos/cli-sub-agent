use super::{ToolAvailabilityState, ToolStatus, ToolTransportDoctorStatus};
use csa_config::ProjectConfig;
use csa_executor::{ClaudeCodeTransport, CodexRuntimeMetadata, CodexTransport};
use std::process::Command;

pub(super) async fn print_tool_availability(config: Option<&ProjectConfig>) {
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

    println!();
    println!("{installed_count}/{total_count} tools ready");
}

pub(super) fn check_tool_status(
    tool_name: &'static str,
    config: Option<&ProjectConfig>,
) -> ToolStatus {
    let binary_name = tool_exe_name(tool_name, config);
    match crate::run_helpers::tool_binary_availability(tool_name, config) {
        crate::run_helpers::ToolBinaryAvailability::Available { .. } => ToolStatus {
            name: tool_name,
            availability: ToolAvailabilityState::Installed,
            binary_name: binary_name.clone(),
            version: check_tool_version(&binary_name),
            hint: None,
            transport: tool_transport_doctor_status(tool_name, config),
        },
        crate::run_helpers::ToolBinaryAvailability::Missing { hint, .. } => ToolStatus {
            name: tool_name,
            availability: ToolAvailabilityState::Missing,
            binary_name,
            version: None,
            hint: Some(hint.into_owned()),
            transport: tool_transport_doctor_status(tool_name, config),
        },
    }
}

pub(super) fn check_tool_version(exe_name: &str) -> Option<String> {
    let output = Command::new(exe_name).arg("--version").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next().map(|s| s.trim().to_string())
}

fn print_tool_status(status: &ToolStatus) {
    for line in render_tool_status_lines(status) {
        println!("{line}");
    }
}

pub(super) fn render_tool_status_lines(status: &ToolStatus) -> Vec<String> {
    let checkmark = if status.is_ready() { "✓" } else { "✗" };
    let status_msg = match status.availability {
        ToolAvailabilityState::Installed => status
            .version
            .as_ref()
            .map(|version| format!("installed ({version})"))
            .unwrap_or_else(|| "installed (version unknown)".to_string()),
        ToolAvailabilityState::Missing => "not found".to_string(),
    };

    let mut lines = vec![format!(
        "{:<12} {} {}",
        format!("{}:", status.name),
        checkmark,
        status_msg
    )];

    if let Some(transport_status) = status.transport.as_ref() {
        lines.push(format!(
            "             Active transport: {}",
            transport_status.transport_active
        ));
        if let Some(acp_compiled_in) = transport_status.acp_compiled_in {
            lines.push(format!(
                "             ACP compiled in: {}",
                yes_no(acp_compiled_in)
            ));
        }
        lines.push(format!(
            "             Probed binary: {}",
            transport_status.probed_binary
        ));
        if let Some(acp_override_hint) = transport_status.acp_override_hint {
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

pub(super) fn tool_status_json(status: &ToolStatus) -> serde_json::Value {
    let mut entry = serde_json::json!({
        "name": status.name,
        "binary": status.binary_name,
        "installed": status.is_ready(),
        "version": status.version,
        "hint": status.hint,
    });

    if let Some(transport_status) = status.transport.as_ref()
        && let Some(object) = entry.as_object_mut()
    {
        object.insert(
            "transport_active".to_string(),
            serde_json::json!(transport_status.transport_active),
        );
        if let Some(acp_compiled_in) = transport_status.acp_compiled_in {
            object.insert(
                "acp_compiled_in".to_string(),
                serde_json::json!(acp_compiled_in),
            );
        }
        object.insert(
            "probed_binary".to_string(),
            serde_json::json!(transport_status.probed_binary),
        );
    }

    entry
}

fn tool_exe_name(tool_name: &str, config: Option<&ProjectConfig>) -> String {
    crate::run_helpers::resolved_tool_binary_name(tool_name, config)
        .unwrap_or(tool_name)
        .to_string()
}

fn tool_transport_doctor_status(
    tool_name: &str,
    config: Option<&ProjectConfig>,
) -> Option<ToolTransportDoctorStatus> {
    match tool_name {
        "codex" => codex_doctor_status(config),
        "claude-code" => claude_code_doctor_status(config),
        _ => None,
    }
}

fn codex_doctor_status(config: Option<&ProjectConfig>) -> Option<ToolTransportDoctorStatus> {
    let transport_active = crate::run_helpers::resolved_codex_transport(config);
    let acp_compiled_in = CodexRuntimeMetadata::acp_compiled_in();

    Some(ToolTransportDoctorStatus {
        transport_active: codex_transport_label(transport_active),
        acp_compiled_in: Some(acp_compiled_in),
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

fn claude_code_doctor_status(config: Option<&ProjectConfig>) -> Option<ToolTransportDoctorStatus> {
    let transport_active = crate::run_helpers::resolved_claude_code_transport(config);

    Some(ToolTransportDoctorStatus {
        transport_active: claude_code_transport_label(transport_active),
        acp_compiled_in: None,
        probed_binary: transport_active.runtime_binary_name().to_string(),
        acp_override_hint: None,
    })
}

fn claude_code_transport_label(transport: ClaudeCodeTransport) -> &'static str {
    match transport {
        ClaudeCodeTransport::Cli => "cli",
        ClaudeCodeTransport::Acp => "acp",
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
