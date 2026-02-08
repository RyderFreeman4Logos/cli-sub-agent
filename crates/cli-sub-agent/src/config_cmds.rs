use anyhow::Result;
use tracing::{error, warn};

use csa_config::init::init_project;
use csa_config::{validate_config, GlobalConfig, ProjectConfig};
use csa_core::types::OutputFormat;

pub(crate) fn handle_config_show(cd: Option<String>, format: OutputFormat) -> Result<()> {
    let project_root = crate::determine_project_root(cd.as_deref())?;
    let config = ProjectConfig::load(&project_root)?
        .ok_or_else(|| anyhow::anyhow!("No configuration found. Run 'csa init' first."))?;

    match format {
        OutputFormat::Json => {
            let json_str = serde_json::to_string_pretty(&config)?;
            println!("{}", json_str);
        }
        OutputFormat::Text => {
            let toml_str = toml::to_string_pretty(&config)?;
            print!("{}", toml_str);
        }
    }
    Ok(())
}

pub(crate) fn handle_config_edit(cd: Option<String>) -> Result<()> {
    let project_root = crate::determine_project_root(cd.as_deref())?;
    let config_path = ProjectConfig::config_path(&project_root);

    if !config_path.exists() {
        error!("Configuration file does not exist. Run 'csa init' first.");
        return Ok(());
    }

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(editor)
        .arg(&config_path)
        .status()?;

    if !status.success() {
        warn!("Editor exited with non-zero status");
    }

    Ok(())
}

pub(crate) fn handle_init(non_interactive: bool, minimal: bool) -> Result<()> {
    let project_root = crate::determine_project_root(None)?;
    let config = init_project(&project_root, non_interactive, minimal)?;
    eprintln!(
        "Initialized project configuration at: {}",
        ProjectConfig::config_path(&project_root).display()
    );
    eprintln!("Project: {}", config.project.name);

    // Generate global config if it doesn't exist
    if let Ok(global_path) = GlobalConfig::config_path() {
        if !global_path.exists() {
            match GlobalConfig::save_default_template() {
                Ok(path) => {
                    eprintln!("Generated global config template at: {}", path.display());
                    eprintln!("  Edit to configure API keys and concurrency limits.");
                }
                Err(e) => {
                    warn!("Failed to generate global config: {}", e);
                }
            }
        }
    }

    Ok(())
}

pub(crate) fn handle_config_validate(cd: Option<String>) -> Result<()> {
    let project_root = crate::determine_project_root(cd.as_deref())?;
    let config = ProjectConfig::load(&project_root)?
        .ok_or_else(|| anyhow::anyhow!("No configuration found. Run 'csa init' first."))?;

    // Check schema version compatibility
    config.check_schema_version()?;

    // Run full validation
    validate_config(&project_root)?;

    eprintln!("Configuration is valid (schema v{})", config.schema_version);
    Ok(())
}
