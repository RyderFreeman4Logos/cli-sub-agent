use anyhow::Result;
use tracing::{error, warn};

use csa_config::init::init_project;
use csa_config::{validate_config, GlobalConfig, ProjectConfig};
use csa_core::types::OutputFormat;

pub(crate) fn handle_config_show(cd: Option<String>, format: OutputFormat) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
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
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
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
    let project_root = crate::pipeline::determine_project_root(None)?;
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

/// Get a raw config value by dotted key path.
///
/// Reads raw TOML files (not the merged/defaulted effective config).
/// Fallback order: project `.csa/config.toml` → global config → `--default`.
/// Use `--project` to skip global, `--global` to skip project.
pub(crate) fn handle_config_get(
    key: String,
    default: Option<String>,
    project_only: bool,
    global_only: bool,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let project_config_path = ProjectConfig::config_path(&project_root);

    // Try project config first (unless --global flag)
    if !global_only {
        match load_and_resolve(&project_config_path, &key) {
            Ok(Some(value)) => {
                println!("{}", format_toml_value(&value));
                return Ok(());
            }
            Ok(None) => {} // Key not found, try next source
            Err(e) => anyhow::bail!(
                "Failed to read project config {}: {e}",
                project_config_path.display()
            ),
        }
    }

    // Try global config (unless --project flag)
    if !project_only {
        match GlobalConfig::config_path() {
            Ok(global_path) => {
                match load_and_resolve(&global_path, &key) {
                    Ok(Some(value)) => {
                        println!("{}", format_toml_value(&value));
                        return Ok(());
                    }
                    Ok(None) => {} // Key not found
                    Err(e) => anyhow::bail!(
                        "Failed to read global config {}: {e}",
                        global_path.display()
                    ),
                }
            }
            Err(e) if global_only && default.is_none() => {
                anyhow::bail!("Cannot determine global config path: {e}");
            }
            Err(_) => {} // Non-critical when falling through to default
        }
    }

    // Fall back to --default or report key not found
    match default {
        Some(d) => {
            println!("{d}");
            Ok(())
        }
        None => anyhow::bail!("Key not found: {key}"),
    }
}

/// Load a TOML file and resolve a dotted key path.
///
/// Returns `Ok(None)` if the file doesn't exist or the key path is absent.
/// Returns `Err` if the file exists but cannot be read or parsed.
fn load_and_resolve(path: &std::path::Path, key: &str) -> Result<Option<toml::Value>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => anyhow::bail!("{e}"),
    };
    let root: toml::Value = content
        .parse()
        .map_err(|e| anyhow::anyhow!("TOML parse error: {e}"))?;
    Ok(resolve_key(&root, key))
}

/// Navigate a TOML value by dotted key path (e.g., "tools.codex.enabled").
fn resolve_key(root: &toml::Value, key: &str) -> Option<toml::Value> {
    let mut current = root;
    for part in key.split('.') {
        current = current.as_table()?.get(part)?;
    }
    Some(current.clone())
}

/// Format a TOML value for stdout (inline for scalars, pretty for tables/arrays).
fn format_toml_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Table(_) | toml::Value::Array(_) => {
            toml::to_string_pretty(value).unwrap_or_else(|_| format!("{value:?}"))
        }
        toml::Value::Datetime(d) => d.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_key_scalar() {
        let root: toml::Value = "[review]\ntool = \"auto\"\n".parse().unwrap();
        let val = resolve_key(&root, "review.tool").unwrap();
        assert_eq!(val.as_str(), Some("auto"));
    }

    #[test]
    fn resolve_key_nested() {
        let root: toml::Value = "[tools.codex]\nenabled = true\n".parse().unwrap();
        let val = resolve_key(&root, "tools.codex.enabled").unwrap();
        assert_eq!(val.as_bool(), Some(true));
    }

    #[test]
    fn resolve_key_missing() {
        let root: toml::Value = "[review]\ntool = \"auto\"\n".parse().unwrap();
        assert!(resolve_key(&root, "nonexistent.key").is_none());
    }

    #[test]
    fn resolve_key_partial_path() {
        let root: toml::Value = "[review]\ntool = \"auto\"\n".parse().unwrap();
        // "review" is a table, not a leaf — resolve_key returns the table
        let val = resolve_key(&root, "review").unwrap();
        assert!(val.is_table());
    }

    #[test]
    fn format_toml_value_string() {
        let v = toml::Value::String("hello".to_string());
        assert_eq!(format_toml_value(&v), "hello");
    }

    #[test]
    fn format_toml_value_integer() {
        let v = toml::Value::Integer(42);
        assert_eq!(format_toml_value(&v), "42");
    }

    #[test]
    fn format_toml_value_bool() {
        let v = toml::Value::Boolean(true);
        assert_eq!(format_toml_value(&v), "true");
    }

    #[test]
    fn load_and_resolve_missing_file() {
        let result = load_and_resolve(std::path::Path::new("/nonexistent/config.toml"), "key");
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn load_and_resolve_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "{{invalid toml").unwrap();
        let result = load_and_resolve(&path, "key");
        assert!(result.is_err());
    }
}

pub(crate) fn handle_config_validate(cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let config = ProjectConfig::load(&project_root)?
        .ok_or_else(|| anyhow::anyhow!("No configuration found. Run 'csa init' first."))?;

    // Check schema version compatibility
    config.check_schema_version()?;

    // Run full validation
    validate_config(&project_root)?;

    eprintln!("Configuration is valid (schema v{})", config.schema_version);
    Ok(())
}
