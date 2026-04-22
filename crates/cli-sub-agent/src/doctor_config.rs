use super::{DoctorEffectiveConfigStatus, DoctorProjectConfigStatus};
use anyhow::Result;
use csa_config::ProjectConfig;
use std::path::Path;

pub(super) fn project_config_tool_lists(
    config: &ProjectConfig,
) -> (Vec<&'static str>, Vec<&'static str>) {
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

pub(super) fn render_project_config_lines(status: &DoctorProjectConfigStatus) -> Vec<String> {
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

pub(super) fn render_effective_config_lines(status: &DoctorEffectiveConfigStatus) -> Vec<String> {
    match status {
        DoctorEffectiveConfigStatus::Defaults => {
            vec!["Effective:   merged config (defaults only)".to_string()]
        }
        DoctorEffectiveConfigStatus::Valid(_) => {
            vec!["Effective:   merged config (valid)".to_string()]
        }
        DoctorEffectiveConfigStatus::Invalid(error) => vec![
            "Effective:   merged config (invalid)".to_string(),
            format!("             Error: {error}"),
        ],
    }
}

pub(super) fn render_tool_availability_error_lines(error: &str) -> Vec<String> {
    vec![
        "Tool availability unknown (effective config invalid)".to_string(),
        format!("Reason:      {error}"),
    ]
}

pub(super) fn load_doctor_project_config_from(
    project_root: &Path,
) -> Result<Option<ProjectConfig>> {
    ProjectConfig::load_project_only(project_root)
}

fn load_doctor_effective_config_from(project_root: &Path) -> Result<Option<ProjectConfig>> {
    ProjectConfig::load(project_root)
}

pub(super) fn inspect_doctor_project_config_from(project_root: &Path) -> DoctorProjectConfigStatus {
    let config_path = project_root.join(".csa").join("config.toml");
    if !config_path.exists() {
        return DoctorProjectConfigStatus::Missing;
    }

    match load_doctor_project_config_from(project_root) {
        Ok(Some(config)) => DoctorProjectConfigStatus::Valid(Box::new(config)),
        Ok(None) => DoctorProjectConfigStatus::Missing,
        Err(error) => DoctorProjectConfigStatus::Invalid(format!("{error:#}")),
    }
}

pub(super) fn inspect_doctor_effective_config_from(
    project_root: &Path,
) -> DoctorEffectiveConfigStatus {
    match load_doctor_effective_config_from(project_root) {
        Ok(Some(config)) => DoctorEffectiveConfigStatus::Valid(Box::new(config)),
        Ok(None) => DoctorEffectiveConfigStatus::Defaults,
        Err(error) => DoctorEffectiveConfigStatus::Invalid(format!("{error:#}")),
    }
}
